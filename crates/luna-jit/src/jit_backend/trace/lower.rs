use super::*;

/// `always_codegen = false` leaves the function undefined in `module`
/// when [`trace_is_enterable`] says nothing will run it; `float_only` as
/// in [`compile_trace_jit`].
pub(super) fn lower_trace_into_inner<M: Module>(
    mut module: &mut M,
    record: &TraceRecord,
    opts: CompileOptions,
    aot_fn_name: Option<&str>,
    always_codegen: bool,
    float_only: bool,
) -> Option<(FuncId, CompiledTrace)> {
    checkpoint("enter");
    if !record.closed {
        checkpoint("bail:not-closed");
        return None;
    }
    checkpoint("post:closed-check");

    // track which AOT data slots
    // we've already `define_data`'d this lower call. `declare_data`
    // returns the same `DataId` for the same name (Cranelift name
    // interning), but `define_data` rejects redefinition with
    // `ModuleError::DuplicateDefinition` — so the dedupe guard sits
    // around `define_data`, not `declare_data`.
    let mut defined_aot_data: std::collections::HashSet<DataId> = std::collections::HashSet::new();

    let head_proto = record.head_proto;
    let max_stack = head_proto.max_stack as usize;
    // Every pass below reads register operands: a constant- or
    // immediate-operand op is lowered as its register form with the
    // constant in virtual register `max_stack` (one past the op's frame,
    // never stored back), whose kind and value `vconsts` holds.
    let translated;
    let (record, vconsts) = match split_const_operands(record, max_stack as u32) {
        Some((t, v)) => {
            translated = t;
            (&translated, v)
        }
        None => (record, Vec::new()),
    };
    let vconst = |i: usize| vconsts.get(i).copied().flatten();
    // a register index past the frame, other than the virtual one
    let oob = |i: usize, r: u32| {
        r as usize >= max_stack && !(r as usize == max_stack && vconst(i).is_some())
    };
    let n = record.ops.len();

    // recorder invariant: the first recorded op is at
    // depth 0 on `head_proto`. A record violating either would break
    // `compute_op_offsets`' depth-bump arithmetic; bail cleanly here
    // rather than panic deeper in.
    if let Some(first) = record.ops.first()
        && (first.inline_depth != 0 || !std::ptr::eq(first.proto.as_ptr(), head_proto.as_ptr()))
    {
        checkpoint("bail:first-op-shape");
        return None;
    }
    checkpoint("post:first-op-check");

    // The smart side-trace gate sits BELOW `compute_op_offsets` so
    // it can reuse the verified op_offsets (calling
    // compute_op_offsets early can panic if the depth invariant
    // fails — verify_depth_invariant runs first).

    // per-op register-window offsets across inlined
    // self-recursive frames. `op_offsets[i]` is the start of op i's
    // register window inside reg_state_buf; `enclosing_call_a[i]` is
    // the matching caller `Op::Call`'s A field (None at depth 0).
    // `window_size` is the largest `off + max_stack` across all ops —
    // sized so even the deepest inlined frame fits. The dispatcher
    // (vm/exec.rs) reads `window_size` off `CompiledTrace` to size
    // its reg_state buffer; only [0..max_stack) is marshalled in
    // from the interp stack, [max_stack..window_size) is zero-init
    // and filled by the trace's own GetUpval / arith.
    // consolidated depth invariant check. Bails if
    // any of:
    //   - first op not at depth 0 (already checked above against
    //     head_proto, but kept here for the pure-function test)
    //   - any consecutive ops jump > 1 depth (e.g. Op::Close
    //     pushing both a Cont::Close frame AND a handler's Lua
    //     frame — recorder sees 0 → 2, IR has no intermediate)
    //   - a depth bump is not preceded by an Op::Call (recorder
    //     contract: only Op::Call can push a new frame)
    //   - any op exceeds MAX_INLINE_DEPTH (the lowerer caps its
    //     window_size on this)
    // The check is pulled into `verify_depth_invariant` (lib
    // unit tested over synthetic depth/Op-is-Call sequences;
    // doesn't need a real `Gc<Proto>`).
    let depth_items: Vec<(u8, bool)> = record
        .ops
        .iter()
        .map(|r| (r.inline_depth, matches!(r.inst.op(), Op::Call)))
        .collect();
    if !verify_depth_invariant(&depth_items) {
        checkpoint("bail:depth-invariant");
        return None;
    }
    checkpoint("post:depth-invariant");
    let (op_offsets, enclosing_call_a) = compute_op_offsets(record);
    let mut window_size: u32 = op_offsets
        .iter()
        .map(|&off| off + max_stack as u32)
        .max()
        .unwrap_or(max_stack as u32);
    // SelfLink close needs `regs_full` to extend through the
    // would-be-next-depth's window so the snapshot-restore copy reads
    // from valid slots. Without this extension, compute_op_offsets
    // only covers the deepest CAPTURED depth (the recorder closed
    // BEFORE pushing the tripping depth's frame), and bump-target
    // reads would go OOB. Extend by one max_stack window past the
    // last Op::Call's bump destination.
    if record.self_link_kind.is_some() {
        let mut last_call_idx: Option<usize> = None;
        for (i, rop) in record.ops.iter().enumerate() {
            if matches!(rop.inst.op(), Op::Call) {
                last_call_idx = Some(i);
            }
        }
        if let Some(idx) = last_call_idx {
            let bump_off = op_offsets[idx] + record.ops[idx].inst.a() + 1;
            let needed = bump_off + max_stack as u32;
            if needed > window_size {
                window_size = needed;
            }
        }
    }
    let window_size_us = window_size as usize;

    // SMART side-trace gate. Compute the child's read-before-write live-
    // in slot set (slots READ without first being WRITTEN within
    // child's body — values carried in from the parent's exit
    // reg_state). Intersect with the parent's body_writes (slots
    // the parent's recorded body writes — values that go STALE
    // across child's internal-loop iters because parent doesn't
    // re-run those writes mid-side-trace). Non-empty intersection
    // = the s12_step_b class of bug; bail compile. Empty = side
    // trace is self-contained w.r.t. parent's writes — safe to
    // internal-loop OR forward-only — allow either.
    //
    // More permissive than banning ALL back-edge ops in side
    // traces: self-contained back-edge side traces (e.g. recursive
    // call branches that re-compute their inputs each iter) can
    // compile and
    // amortize the parent's hot-exit dispatch cost.
    if let Some((parent_proto, parent_head_pc, _)) = record.side_trace_parent {
        // Check 1: any back-edge op? (ForLoop / TForLoop / Jmp -bx)
        let has_back_edge = record.ops.iter().any(|op| match op.inst.op() {
            luna_core::vm::isa::Op::ForLoop | luna_core::vm::isa::Op::TForLoop => true,
            luna_core::vm::isa::Op::Jmp => op.inst.sbx() < 0,
            _ => false,
        });
        if has_back_edge {
            // Back-edge means the trace's IR will internal-loop OR
            // re-execute body ops. Two correctness requirements:
            //
            //   (a) child must not READ a slot the parent's body
            //       writes without first writing it itself (the
            //       s12_step_b stale-register bug).
            //   (b) child must not contain side-effect-producing
            //       ops (Call / TForCall / SetTable / SetI /
            //       SetField / SetUpval / SetTabUp / Closure /
            //       Close / Tbc) — these advance shared heap /
            //       iterator state that interp re-observes after
            //       the side trace returns, causing double-effect
            //       (the s12_step_d TForCall-double-advance bug).
            let has_impure = record.ops.iter().any(|op| {
                use luna_core::vm::isa::Op;
                matches!(
                    op.inst.op(),
                    Op::Call
                        | Op::TailCall
                        | Op::TForCall
                        | Op::SetTable
                        | Op::SetI
                        | Op::SetField
                        | Op::SetUpval
                        | Op::SetTabUp
                        | Op::Closure
                        | Op::Close
                        | Op::Tbc
                )
            });
            if has_impure {
                checkpoint("bail:side-trace-back-edge-with-impure");
                return None;
            }
            // Pure back-edge trace: still check live-in vs parent
            // writes (Add/Move loops can still re-read a stale
            // parent-written slot each iter).
            let child_live_in = compute_live_in_slots(record, &op_offsets);
            if !child_live_in.is_empty() {
                let parent_writes_opt = {
                    let traces = parent_proto.traces.borrow();
                    traces
                        .iter()
                        .find(|t| t.head_pc == parent_head_pc)
                        .map(|pct| pct.body_writes.clone())
                };
                if let Some(parent_writes) = parent_writes_opt {
                    let mut i = 0;
                    let mut j = 0;
                    let pw = &parent_writes[..];
                    let cl_li = &child_live_in[..];
                    while i < pw.len() && j < cl_li.len() {
                        match pw[i].cmp(&cl_li[j]) {
                            std::cmp::Ordering::Equal => {
                                checkpoint("bail:side-trace-live-in-overlap");
                                return None;
                            }
                            std::cmp::Ordering::Less => i += 1,
                            std::cmp::Ordering::Greater => j += 1,
                        }
                    }
                } else {
                    checkpoint("bail:side-trace-parent-ct-missing");
                    return None;
                }
            }
            // All back-edge checks passed; trace is allowed.
        }
        // No back-edge: forward-only is always safe (single-iter
        // execution; no internal-loop semantics to break).
    }
    checkpoint("post:side-trace-v2e-smart-gate");

    // per-inlined-frame metadata for the
    // frame-mat helper. Walk record.ops; every self-recursive
    // Op::Call (next op at depth+1 on the same proto) describes one
    // callee frame the helper will push at side-exit time.
    //
    // Bail when:
    //   - any self-recursive Call has C != 2 (i.e. nresults != 1) —
    //     the Op::Return1 copy-back assumes one return value
    //   - the head closure's proto is vararg — helper doesn't
    //     reconstruct the vararg rotation that `push_frame` does
    //
    // frame-mat data is per-cmp-site: a single global
    // indexed-by-depth array gives the wrong chain to sibling-Call
    // branches and loops fib forever. Per-site `per_exit_metas` is built BELOW after
    // `cmp_dirs` are populated — that pass needs the cmp direction
    // to compute each site's side-exit PC.
    //
    // Pre-emit validation here: bail any self-recursive Call whose
    // `C != 2` (nresults != 1) — the `Op::Return1` copy-back
    // assumes one return value and the materialize helper bakes
    // whatever the meta says without validating.
    for (i, rop) in record.ops.iter().enumerate() {
        if !matches!(rop.inst.op(), Op::Call) {
            continue;
        }
        let depth = rop.inline_depth as usize;
        let Some(next) = record.ops.get(i + 1) else {
            continue;
        };
        if (next.inline_depth as usize) != depth + 1 {
            continue;
        }
        if !std::ptr::eq(next.proto.as_ptr(), head_proto.as_ptr()) {
            continue;
        }
        // accept Call C=2 (single ret)
        // OR Call C=0 with var_count snapshot == 1 (multi-return
        // form that happens to return exactly 1 value, e.g.
        // binary_trees `make`'s `return {...}`). Both reduce to the
        // same emit (single-value Return1 copy-back from callee to
        // caller). For C=0 with var_count != 1, bail — multi-value
        // copy-back is unsupported.
        let c = rop.inst.c();
        if c == 2 {
            // OK, single return
        } else if c == 0 && rop.var_count == Some(1) {
            // OK, single return via variable form
        } else {
            checkpoint("bail:self-rec-Call-c-not-1");
            return None;
        }
    }
    checkpoint("post:self-rec-Call-validate");
    // also bail if the head proto is vararg.
    // The materialize helper builds frames with `n_varargs = 0`,
    // which doesn't reconstruct the vararg-rotated layout that
    // `push_frame` lays out for vararg functions. fib + simple
    // self-recursion isn't vararg.
    if head_proto.is_vararg {
        for r in &record.ops {
            if r.inline_depth > 0 {
                checkpoint("bail:vararg-head-with-depth");
                return None;
            }
        }
    }
    checkpoint("post:vararg-check");

    // Find the first trace-terminating op. Two species:
    //
    // - `Op::Call` *truncates* the trace: every op before
    //   gets normal IR, the Call emits a side-exit at its own PC,
    //   and every op after is dropped.
    // - `Op::ForLoop` is the numeric-for back-edge: every
    //   op before is the loop body, the ForLoop emits its own cmp
    //   + step + brif at the tail. Continue branch is the internal
    //   back-edge (or `return head_pc` in one-shot mode); side-exit
    //   branch returns `pc + 1` so interp resumes past the loop.
    //
    // Whichever appears *first* in the recorded ops takes the tail
    // slot — the other (if any) lives in the dropped region and is
    // ignored.
    // Scan for math folds first — a `Call` that's part of a fold
    // doesn't truncate the trace.
    let mut folded_ops: Vec<bool> = vec![false; n];
    let mut math_folds: Vec<TraceMathFold> = Vec::new();
    {
        // Recogniser does its own internal bounds check per arm
        // (libm1 needs 4 ops, min2/max2 needs ≥3 ops with the
        // Call no more than `MINMAX_FOLD_ARG_PREP_MAX` past
        // `start_idx + 1`). Walk every index; the matcher returns
        // None when the window doesn't fit so we don't run off
        // the end of `record.ops`.
        //
        // `folded_ops` bitmap layout:
        //   * `Libm1` — flags 4 consecutive indices
        //     `start_idx..=start_idx+3` (GetTabUp, GetField, Move,
        //     Call). All emit silently except `start_idx` which
        //     fires the libm call.
        //   * `Min2 / Max2` — flags only `start_idx`,
        //     `start_idx + 1`, and `call_idx`. Arg-prep ops in
        //     between are NOT folded — they execute normally so
        //     the Call args land at R[A+1] / R[A+2] by the
        //     standard Lua Call ABI. The Call's emit position
        //     fires `fmin / fmax`.
        let mut i = 0;
        while i < n {
            if let Some(fold) = try_match_trace_math_fold(record, i, head_proto, opts.pre53) {
                match fold.kind {
                    FoldKind::Libm1 => {
                        for k in 0..4 {
                            folded_ops[i + k] = true;
                        }
                        i += 4;
                    }
                    FoldKind::Min2 | FoldKind::Max2 => {
                        folded_ops[fold.start_idx] = true;
                        folded_ops[fold.start_idx + 1] = true;
                        folded_ops[fold.call_idx] = true;
                        // Advance past the Call so the next scan
                        // starts beyond the recognised fold.
                        i = fold.call_idx + 1;
                    }
                }
                math_folds.push(fold);
            } else {
                i += 1;
            }
        }
    }

    // the terminator scan also accounts for
    // inline self-recursion. Self-recursive Op::Call (next op is at
    // depth+1 on the same proto, within MAX_INLINE_DEPTH) is NOT a
    // terminator — body emit walks past it and op_offsets shifts the
    // register window for the callee.
    //
    // cmp@d>0 does not close via InlineAbort:
    // body emit calls the frame-mat helper at the side-exit then
    // returns side_exit_pc; dispatcher's restore loop walks the
    // newly-pushed inline frames. ForLoop@d>0 / non-self Call@d>0 /
    // depth past MAX_INLINE_DEPTH / proto mismatch still close via
    // InlineAbort.
    //
    // Op::Return0/Return1 at depth=0 terminates
    // the trace via `TraceEnd::Return` (caller frame unwind). The
    // recorder closes the trace cleanly past the return; without
    // this truncation the Return would fail the whitelist check and
    // bail the whole compile.
    // DownRec close wins over SelfLink: a depth>0
    // `Op::Return` re-trip of the recunroll threshold (captured at
    // `exec.rs` recorder gate) routes here BEFORE the SelfLink arm.
    //
    // effective_end for DownRec is the index of the natural
    // terminator (depth-0 Return/Call/ForLoop) so the body emit's
    // whitelist gate doesn't bail on the final op. Using
    // `record.ops.len()` would walk the natural close op as a body
    // op, which fails the `is_whitelisted_op` check (Op::Return at
    // depth 0 is end-shape, not body) and aborts compile before the
    // DownRec tail arm can fire. The scan mirrors TraceEnd::Return /
    // Call's picker shape and falls back to `record.ops.len()` only
    // when no natural terminator is present.
    let plain_end = plain_trace_end(record, &folded_ops, head_proto);
    let end_idx_opt: Option<(usize, TraceEnd)> = if let Some(dr) = record.downrec_close {
        let mut natural_end = record.ops.len();
        for (i, r) in record.ops.iter().enumerate() {
            if folded_ops[i] {
                continue;
            }
            let depth = r.inline_depth as usize;
            if depth != 0 {
                continue;
            }
            match r.inst.op() {
                Op::Call | Op::ForLoop | Op::TForLoop | Op::Return0 | Op::Return1 => {
                    natural_end = i;
                    break;
                }
                _ => {}
            }
        }
        if plain_end.is_some_and(|(i, _)| i < natural_end) {
            checkpoint("bail:downrec-body-terminator");
            return None;
        }
        Some((
            natural_end,
            TraceEnd::DownRec {
                return_pc: dr.return_pc,
                target_proto_id: dr.target_proto.as_ptr() as usize,
                depth_delta: dr.depth_delta,
            },
        ))
    } else if let Some(kind) = record.self_link_kind {
        // Self-link close overrides the natural terminator
        // scan. Recorder stopped capturing AT the head_pc re-entry
        // (about to re-execute the deepest-inlined frame's first op);
        // every prior op is intentional inline body. effective_end =
        // record.ops.len() so body emit walks the whole captured trail.
        // The tail emit picks the SelfLink arm (snapshot-restore +
        // branch-to-self) instead of any natural terminator that might
        // happen to sit at the end (e.g., a depth>0 Return in fib's
        // post-recursion add path that the recorder never actually
        // reaches in the cycle catch — but we guard against it anyway).
        // Every op before the trailing Call must be one the plain scan
        // walks past: the body emit has no lowering for a loop edge, a
        // non-inlined call or a return in the middle of a trace.
        match plain_end {
            None => {}
            Some((i, _)) if i + 1 == n && matches!(record.ops[i].inst.op(), Op::Call) => {}
            Some(_) => {
                checkpoint("bail:self-link-body-terminator");
                return None;
            }
        }
        Some((n, TraceEnd::SelfLink(kind)))
    } else {
        plain_end
    };
    let effective_end = end_idx_opt.map(|(i, _)| i).unwrap_or(n);
    // escape analysis over the recorded body +
    // terminator. The pre-emit pass below demotes any Sinkable
    // site that doesn't meet the sunk-emit criteria back to
    // Escaped, so emit only honours sites we actually allocate
    // virt-slot Variables for.
    checkpoint("post:end-idx-found");
    let mut escape = escape_analyze(
        record,
        effective_end,
        end_idx_opt.map(|(_, k)| k),
        head_proto,
    );
    checkpoint("post:escape-analyze");

    // `flush_ctx` is declared mut here so
    // the entry-block setup below can populate it with
    // `Some(FlushCtx { ... })` when an active_accum is detected.
    // The 19 `emit_store_back_and_return_*` call sites all read
    // `flush_ctx.as_ref()`; the helpers no-op when it's None.
    let mut flush_ctx: Option<FlushCtx> = None;

    // detect the FIRST Bufferable AccumSite.
    // Buffered emit handles a single site. The 4 idiom op indices
    // are pre1 = op_idx-2, pre2 = op_idx-1, concat = op_idx,
    // post = op_idx+1.
    #[derive(Clone, Copy, Debug)]
    struct BufferedAccum {
        accum_slot: u32,
        piece_slot: u32,
        pre1_idx: usize,
        pre2_idx: usize,
        concat_idx: usize,
        post_idx: usize,
    }
    // Keep the old names working for the per-tail paths below.
    let call_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::Call)) => Some(i),
        _ => None,
    };
    let for_loop_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::ForLoop)) => Some(i),
        _ => None,
    };
    let inline_abort_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::InlineAbort)) => Some(i),
        _ => None,
    };
    let return_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::Return)) => Some(i),
        _ => None,
    };
    // `self_link_idx_opt = Some(effective_end)` when this is a
    // self-link close. Used to gate the new tail emit arm + override
    // `do_internal_loop` (the trace is designed to loop).
    let self_link_idx_opt: Option<(usize, SelfRecKind)> = match end_idx_opt {
        Some((i, TraceEnd::SelfLink(kind))) => Some((i, kind)),
        _ => None,
    };
    // `downrec_idx_opt` = `Some(effective_end, return_pc,
    // target_proto_id, depth_delta)` when the down-rec catch tripped.
    let downrec_idx_opt: Option<(usize, u32, usize, u8)> = match end_idx_opt {
        Some((
            i,
            TraceEnd::DownRec {
                return_pc,
                target_proto_id,
                depth_delta,
            },
        )) => Some((i, return_pc, target_proto_id, depth_delta)),
        _ => None,
    };

    let has_cmp = record.ops[..effective_end]
        .iter()
        .any(|r| matches!(r.inst.op(), Op::Lt | Op::Le | Op::Eq));
    // SelfLink close ALSO permits internal loop (in fact it
    // REQUIRES it — the whole point of the trace is to loop with
    // bump-base + branch-to-self). Treat self_link_idx_opt the same as
    // the cmp/ForLoop loop-permission predicates, and (critically)
    // don't trip on inline_abort_idx_opt — the SelfLink close path
    // doesn't go through that arm.
    // `downrec_idx_opt` does NOT permit internal loop: its tail
    // either stitches through the dispatcher or deopts, so DownRec
    // is a hard truncation marker that forces one-shot dispatch.
    let do_internal_loop = opts.internal_loop
        && (has_cmp || for_loop_idx_opt.is_some() || self_link_idx_opt.is_some())
        && call_idx_opt.is_none()
        && return_idx_opt.is_none()
        && inline_abort_idx_opt.is_none()
        && downrec_idx_opt.is_none();
    // The head-frame registers the dispatcher checks on entry (see
    // `entry_live`); the others start held on the stack.
    let parent_exit_tags = side_parent_exit_tags(record);
    let head_live = entry_live(
        record,
        &op_offsets,
        effective_end,
        max_stack,
        do_internal_loop,
        parent_exit_tags.as_deref(),
    );
    let active_accum: Option<BufferedAccum> = escape
        .accum_sites
        .iter()
        .find(|s| s.state == BufferState::Bufferable)
        // the buffer reads both as strings on the entry check's word
        .filter(|s| head_live[s.accum_slot as usize] && head_live[s.piece_slot as usize])
        .map(|s| BufferedAccum {
            accum_slot: s.accum_slot,
            piece_slot: s.piece_slot,
            pre1_idx: s.op_idx - 2,
            pre2_idx: s.op_idx - 1,
            concat_idx: s.op_idx,
            post_idx: s.op_idx + 1,
        });

    // Pre-emit verification. Any op outside the whitelist contract
    // bails so the trace becomes a no-op (the recorder counts it
    // toward the head PC's failure count and won't re-record
    // unless the back-edge counter rolls over again).
    //
    // `consumed_by_cmp[i] = true` marks ops[i] as a `Jmp` whose
    // sole role is to be the recorded post-cmp branch — the cmp's
    // `brif` already carries its control transfer, so emit skips
    // the Jmp's IR entirely.
    let mut consumed_by_cmp = vec![false; effective_end];
    // Parallel to record.ops: for each cmp op, which direction
    // was recorded? `None` for non-cmps; set by the pre-emit pass
    // and consumed by body emit.
    let mut cmp_dirs: Vec<Option<CmpDir>> = vec![None; effective_end];
    checkpoint("pre:cmp-dirs-loop");
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        // Folded math-fold ops are validated by the matcher above;
        // the per-op contract here would reject GetTabUp /
        // GetField / Move (dst > A semantic) so skip them.
        if folded_ops[i] {
            continue;
        }
        // depth>0 ops are allowed inside the inline
        // self-recursion path. `end_idx_opt` already guards the path
        // (cmp@d>0 / ForLoop@d>0 / non-self Call / proto mismatch /
        // depth past MAX_INLINE_DEPTH all close the trace before they
        // hit emit), so any op reaching this point with depth>0 is
        // a same-proto inline body op the lowerer can handle.
        // capture op_id BEFORE per-op checks for
        // failure-phase narrowing.
        set_last_op_id(rop.inst.op() as u8);
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            checkpoint("bail:cmp-dirs-cross-proto-op");
            return None;
        }
        let op = rop.inst.op();
        // self-recursive Op::Call inside the inline
        // path emits no IR (the next op shifts to the callee window
        // via op_offsets). It's not in `is_whitelisted_op`, so
        // accept it explicitly when depth>0 OR when the next op is
        // at depth+1 (the recorder's self-recursive marker).
        if matches!(op, Op::Call) {
            // The terminator pass already let this Op::Call past as
            // a self-recursive call. Skip the whitelist check.
            continue;
        }
        // Op::Return0 / Op::Return1 at depth>0 are
        // the inline path's unwind ops. They're not in the
        // whitelist (it only covers depth=0 ops with no return
        // semantics); admit them when depth>0.
        if rop.inline_depth > 0 && matches!(op, Op::Return0 | Op::Return1) {
            // Bound the A operand for Return1 — Return0 has no A read.
            if matches!(op, Op::Return1) && (rop.inst.a() as usize) >= max_stack {
                checkpoint("bail:cmp-dirs-Return1-a-oob");
                return None;
            }
            continue;
        }
        if !is_whitelisted_op(op) {
            checkpoint("bail:cmp-dirs-op-not-whitelisted");
            return None;
        }
        // Op::GetField is lowered standalone via
        // luna_jit_table_get_field (string key from Proto.consts).
        // Op::GetTabUp also lowered standalone via
        // luna_jit_op_get_tab_up. Both require K[C] = Str at compile
        // time (the const-pool string key is baked into IR). GetTabUp
        // additionally pins B = upvalue index in the trace head
        // closure; the helper resolves it via JIT_CL TLS at runtime.
        if matches!(op, Op::GetTabUp) {
            let cx = rop.inst.c() as usize;
            if cx >= head_proto.consts.len()
                || !matches!(head_proto.consts[cx], luna_core::runtime::Value::Str(_))
            {
                checkpoint("bail:cmp-dirs-GetTabUp-key-not-str");
                return None;
            }
        }
        // Op::SetField uses K[B] as string key,
        // Op::GetField uses K[C]. Pre-emit verifies the const is Str.
        if matches!(op, Op::SetField) {
            let bx = rop.inst.b() as usize;
            if bx >= head_proto.consts.len()
                || !matches!(head_proto.consts[bx], luna_core::runtime::Value::Str(_))
            {
                return None;
            }
        }
        if matches!(op, Op::GetField) {
            let cx = rop.inst.c() as usize;
            if cx >= head_proto.consts.len()
                || !matches!(head_proto.consts[cx], luna_core::runtime::Value::Str(_))
            {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        let ins = rop.inst;
        let a = ins.a() as usize;
        let b = ins.b() as usize;
        let c = ins.c() as usize;
        match op {
            Op::Call => {
                unreachable!("Op::Call only appears at effective_end (truncation guarded above)")
            }
            Op::ForLoop => {
                unreachable!("Op::ForLoop only appears at effective_end (loop-end guarded above)")
            }
            Op::TForLoop => unreachable!(
                "Op::TForLoop only appears at effective_end (close-on-back-edge guarded above)"
            ),
            Op::TForPrep => {
                // generic-for prep: forward `add_pc(bx)`
                // to the body-tail (TForCall). Recorder enters at
                // body_top = head_pc, AFTER TForPrep, so the record
                // body never sees TForPrep in normal pickup; bail if
                // it shows up (mid-body / inline-depth>0 = unsupported
                // shape).
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            Op::TForCall => {
                // generic-for body tail. Calls iter
                // via the `luna_jit_op_tforcall` helper. Bounds:
                // helper accesses R[A..A+7] (gen/state/ctrl plus the
                // generator-call window R[A+4..A+6] + space for the
                // first two returns). Restrict to inline_depth = 0
                // (helper reads vm.stack via the trace head's frame
                // base; inline frames aren't pushed during trace IR
                // execution). C field = nvars in [1, 250) per PUC.
                if rop.inline_depth > 0 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if a + 6 >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let nvars = ins.c();
                if nvars == 0 || nvars > 250 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::Concat => {
                // N-operand right-associative concat.
                // Helper reads vm.stack[base+A..base+A+B); spill all
                // operand slots in body emit. Restrict to depth=0
                // (helper resolves base via trace head's Lua frame).
                if rop.inline_depth > 0 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let n_operands = ins.b() as usize;
                if n_operands < 2 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                match a.checked_add(n_operands) {
                    Some(end) if end <= max_stack => {}
                    _ => return None,
                }
            }
            Op::GetTabUp => {
                // standalone GetTabUp body bounds.
                // K[C] = Str validated by the upstream cmp-dirs gate;
                // here we just bounds-check the A register.
                if a >= max_stack {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            Op::SetField | Op::GetField => {
                // validated above (Str const at K[B] or
                // K[C] respectively); bounds-check the reg operands
                // here.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if matches!(op, Op::SetField) && c >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if matches!(op, Op::GetField) && b >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::Jmp => {
                // Validated in the second pass below.
            }
            Op::Move => {
                if a >= max_stack || b >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::LoadI => {
                // R[A] := signed-bx immediate. No reg operand
                // beyond A; sBx fits in i32 (decoded from u32 by
                // Inst::sbx) so the i64 conversion is lossless.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::LoadF => {
                // R[A] := signed-bx immediate as f64.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::LoadNil => {
                // R[A..=A+B] := nil. Validate the full
                // range fits in window; emit pass writes iconst(0)
                // per slot.
                match a.checked_add(b) {
                    Some(end) if end < max_stack => {}
                    _ => return None,
                }
            }
            Op::Close => {
                // close open upvals at slot ≥ A.
                // Limited to inline_depth=0 (helper reads vm.stack
                // via the trace-head frame's base; inline frames aren't
                // pushed). Bounds check on A.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if rop.inline_depth > 0 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::Closure => {
                // R[A] := closure(proto.protos[Bx]).
                // Shared-upval / 0-upval closures, plus in_stack
                // upval support via per-upval pre-Closure
                // spill (emit writes vm.stack[base + d.index] from
                // regs[d.index] before calling op_closure helper).
                //
                // Restrictions:
                // - depth = 0 only: spill writes vm.stack via the
                //   trace-head frame's `base`; inline frames (depth>0)
                //   aren't pushed during trace IR execution, so a
                //   spill at depth>0 would target wrong slots.
                // - Source slot must have a known RegKind (not Unset):
                //   spill needs a tag to pack the i64 payload back to
                //   a Value. Unset would mean trace never wrote the
                //   slot AND entry_tags didn't snapshot it.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if rop.inline_depth > 0 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let bx = ins.bx() as usize;
                if bx >= head_proto.protos.len() {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let inner = head_proto.protos[bx];
                for d in inner.upvals.iter() {
                    if !d.in_stack {
                        continue;
                    }
                    let src_idx = d.index as usize;
                    if src_idx >= max_stack {
                        {
                            checkpoint("bail:cmp-dirs-body-other");
                            return None;
                        }
                    }
                }
            }
            Op::LoadK => {
                // R[A] := proto.consts[Bx]. Step-8 only lowers
                // Int / Float consts; Str / Bool / Nil need a
                // wider marshalling story.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let bx = ins.bx() as usize;
                if bx >= head_proto.consts.len() {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if !matches!(
                    head_proto.consts[bx],
                    luna_core::runtime::Value::Int(_) | luna_core::runtime::Value::Float(_)
                ) {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => {
                if a >= max_stack || oob(i, b as u32) || oob(i, c as u32) {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            // 3-reg Int arith / bitwise ops — same bounds rules as
            // Add/Sub/Mul. Operand-type assumed Int (recorder is
            // trusted); Float / mixed paths would need RegKind
            // tracking like the method JIT.
            Op::IDiv | Op::Mod | Op::BAnd | Op::BOr | Op::BXor | Op::Shl | Op::Shr => {
                if a >= max_stack || oob(i, b as u32) || oob(i, c as u32) {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            // 2-reg unary: `R[A] := op R[B]` — Unm (negation),
            // BNot (bitwise NOT).
            Op::Unm | Op::BNot => {
                if a >= max_stack || b >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            // `if (R[A] == const[B]) ~= K then pc++` — same
            // cmp-then-Jmp shape as Lt/Le/Eq. Const RHS is either
            // an Int (icmp eq) or a Float (fcmp eq).
            Op::EqK => {
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let bx = ins.b() as usize;
                if bx >= head_proto.consts.len() {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if !matches!(
                    head_proto.consts[bx],
                    luna_core::runtime::Value::Int(_) | luna_core::runtime::Value::Float(_)
                ) {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                // EqK pairs with the same trailing Jmp at
                // cmp_pc + 1 contract as Lt/Le/Eq.
                if i + 1 >= effective_end {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let next = &record.ops[i + 1];
                if !matches!(next.inst.op(), Op::Jmp) || next.pc != rop.pc + 1 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                consumed_by_cmp[i + 1] = true;
            }
            Op::Test => {
                // `if (not R[A] == k) then pc++`. Same
                // direction inference as cmp ops: next.pc==pc+1 + Jmp
                // → TookJmp (test failed); next.pc==pc+2 → SkippedJmp
                // (test passed, K matched).
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if i + 1 >= effective_end {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let next = &record.ops[i + 1];
                let took_jmp = matches!(next.inst.op(), Op::Jmp) && next.pc == rop.pc + 1;
                let skipped_jmp = next.pc == rop.pc + 2;
                if took_jmp {
                    consumed_by_cmp[i + 1] = true;
                    cmp_dirs[i] = Some(CmpDir::TookJmp);
                } else if skipped_jmp {
                    let slot = (rop.pc + 1) as usize;
                    let jmp_inst = head_proto.code.get(slot).copied();
                    if !jmp_inst.is_some_and(|x| matches!(x.op(), Op::Jmp)) {
                        {
                            checkpoint("bail:cmp-dirs-body-other");
                            return None;
                        }
                    }
                    cmp_dirs[i] = Some(CmpDir::SkippedJmp);
                } else {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::TestSet => {
                // `if R[B].truthy() == K then R[A]=R[B]
                // else pc++`. R[B] is source; R[A] is move target on
                // test-pass. Direction encoding inverted vs Op::Test:
                //   TookJmp (pc+1 = Jmp) = test passed (no pc++)
                //   SkippedJmp (pc+2)    = test failed (pc++)
                if a >= max_stack || b >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if i + 1 >= effective_end {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                let next = &record.ops[i + 1];
                let took_jmp = matches!(next.inst.op(), Op::Jmp) && next.pc == rop.pc + 1;
                let skipped_jmp = next.pc == rop.pc + 2;
                if took_jmp {
                    consumed_by_cmp[i + 1] = true;
                    cmp_dirs[i] = Some(CmpDir::TookJmp);
                } else if skipped_jmp {
                    let slot = (rop.pc + 1) as usize;
                    let jmp_inst = head_proto.code.get(slot).copied();
                    if !jmp_inst.is_some_and(|x| matches!(x.op(), Op::Jmp)) {
                        {
                            checkpoint("bail:cmp-dirs-body-other");
                            return None;
                        }
                    }
                    cmp_dirs[i] = Some(CmpDir::SkippedJmp);
                } else {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::Lt | Op::Le | Op::Eq => {
                if oob(i, a as u32) || oob(i, b as u32) {
                    {
                        checkpoint("bail:cmp-ab-oob");
                        return None;
                    }
                }
                // Direction inference: peek at the next recorded
                // op's PC relative to the cmp's PC.
                //
                // - next.pc == cmp.pc + 1 (and it's a Jmp): cmp
                //   matched K → fell through to Jmp → executed it.
                //   `TookJmp` direction. consumed_by_cmp marks
                //   the Jmp.
                // - next.pc == cmp.pc + 2: cmp didn't match K →
                //   pc++ skipped the Jmp slot → continued at
                //   pc + 2. `SkippedJmp` direction. The skipped
                //   Jmp isn't in record.ops; we read its target
                //   from `head_proto.code[cmp.pc + 1]` for the
                //   side-exit PC.
                // - Anything else: bail (cross-block path the
                //   lowerer can't model).
                // relax to `record.ops.len()`:
                // if the Cmp is at `effective_end - 1`, the
                // terminator at `record.ops[effective_end]`
                // still gives us a pc we can use for direction
                // inference (took_jmp / skipped_jmp). Only bail
                // when there's literally no next recorded op.
                // The took_jmp path's `consumed_by_cmp[i+1]`
                // write is gated by `i + 1 < effective_end` so
                // we don't mark a terminator op as consumed.
                if i + 1 >= record.ops.len() {
                    {
                        checkpoint("bail:cmp-at-record-end");
                        return None;
                    }
                }
                let next = &record.ops[i + 1];
                let took_jmp = matches!(next.inst.op(), Op::Jmp) && next.pc == rop.pc + 1;
                let skipped_jmp = next.pc == rop.pc + 2;
                if took_jmp {
                    if i + 1 < effective_end {
                        consumed_by_cmp[i + 1] = true;
                    }
                    cmp_dirs[i] = Some(CmpDir::TookJmp);
                } else if skipped_jmp {
                    // Verify the slot we'd resume to (the Jmp)
                    // is actually a Jmp in the Proto's bytecode.
                    let slot = (rop.pc + 1) as usize;
                    let jmp_inst = head_proto.code.get(slot).copied();
                    if !jmp_inst.is_some_and(|x| matches!(x.op(), Op::Jmp)) {
                        {
                            checkpoint("bail:cmp-skipped-but-no-jmp-slot");
                            return None;
                        }
                    }
                    cmp_dirs[i] = Some(CmpDir::SkippedJmp);
                } else {
                    {
                        checkpoint("bail:cmp-next-pc-mismatch");
                        return None;
                    }
                }
            }
            // Table ops — A is the dest / table reg per op; B/C may be
            // immediates (SetI's key, GetI's key, NewTable's hints).
            Op::NewTable => {
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::GetI => {
                // R[A] := R[B][C_imm]
                if a >= max_stack || b >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::GetTable => {
                // R[A] := R[B][R[C]]
                if a >= max_stack || b >= max_stack || c >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::SetI => {
                // R[A][B_imm] := R[C]
                if a >= max_stack || c >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::SetTable => {
                // R[A][R[B] or const[B]] := R[C] or const[C].
                // Step-6 only handles the all-reg form (k=false);
                // const RHS goes through different helpers.
                if ins.k() {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if a >= max_stack || b >= max_stack || c >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::SetList => {
                // R[A][C + i] = R[A + i] for i in 1..=B.
                // Step-7 only handles the fixed-count form
                // (B > 0) without the k=true ExtraArg follower
                // (which encodes a >MAX_ABC offset). The element
                // window must fit in the frame.
                if ins.k() || ins.b() == 0 {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if a >= max_stack || a + ins.b() as usize >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::Len => {
                // R[A] := #R[B]
                if a >= max_stack || b >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            Op::GetUpval => {
                // R[A] := UpVal[B]. The upval index B is bounded by
                // head_proto.upvals.len() at compile time.
                if a >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                if b >= head_proto.upvals.len() {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
            _ => unreachable!("whitelist gated above"),
        }
    }
    // Jmp validation inside the normal range. A Jmp is OK if it
    // was consumed by a preceding cmp (handled above) or sits at
    // the effective end's last position (the back-edge that closes
    // the loop, or the slot right before an Op::Call truncation —
    // the tail / side-exit emits the control transfer).
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        if matches!(rop.inst.op(), Op::Jmp) && !consumed_by_cmp[i] && i + 1 != effective_end {
            return None;
        }
    }

    // Validate the truncating Op::Call (if any). Self-recursion is
    // not verified — the recorder is trusted to only feed sound
    // patterns.
    if let Some(call_idx) = call_idx_opt {
        // call_idx_opt only set for non-self
        // Op::Call at depth 0 (self-recursive inline calls pass
        // through end_idx_opt without truncating; depth>0 closures
        // close via TraceEnd::InlineAbort), so the depth check below
        // is only a debug assert.
        let rop = &record.ops[call_idx];
        debug_assert_eq!(rop.inline_depth, 0, "TraceEnd::Call only at depth 0");
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        let a = rop.inst.a() as usize;
        if a >= max_stack {
            return None;
        }
    }

    // Validate the trailing Op::ForLoop (if any). Step-6 only
    // lowers the 5.4+ Int count form — pre-5.3 compares R[A+1]
    // directly against `limit` and uses a different state slot
    // layout, so traces from those dialects bail. The recorder
    // is trusted that R[A..A+3] really do hold Ints at runtime;
    // the dispatcher's all-Int marshal gate enforces that
    // separately on the call boundary.
    // validate Op::Return0/Return1 at depth=0
    // (TraceEnd::Return). Same A bound rule as Call truncation
    // applies to Return1; Return0 has no A read.
    if let Some(return_idx) = return_idx_opt {
        let rop = &record.ops[return_idx];
        debug_assert_eq!(rop.inline_depth, 0, "TraceEnd::Return only at depth 0");
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        if matches!(rop.inst.op(), Op::Return1) {
            let a = rop.inst.a() as usize;
            if a >= max_stack {
                return None;
            }
        }
    }

    if let Some(for_loop_idx) = for_loop_idx_opt {
        let rop = &record.ops[for_loop_idx];
        debug_assert_eq!(rop.inline_depth, 0, "TraceEnd::ForLoop only at depth 0");
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        let a = rop.inst.a() as usize;
        match rop.inst.op() {
            Op::ForLoop => {
                if opts.pre53 {
                    return None;
                }
                // ForLoop touches R[A], R[A+1] (count), R[A+2] (step),
                // R[A+3] (visible loop var). All must fit in the frame.
                if a + 3 >= max_stack {
                    return None;
                }
                // Bail on Float ForLoop. Trace JIT's emit at line ~7233
                // reads R[A+1] as Int count + tests `count > 0`. For
                // Float ForLoop (5.4+ Float-counter form), R[A+1] is
                // the LIMIT (Float bits), not a remaining-iteration
                // count. The Int-semantics check would treat the float
                // bits as a large positive integer (always > 0) and
                // loop forever inside the trace. PUC's interp handles
                // Float and Int ForLoop with separate semantics; the
                // trace JIT only emits the Int path correctly.
                if a < record.entry_tags.len()
                    && record.entry_tags[a] == luna_core::runtime::value::raw::FLOAT
                {
                    return None;
                }
            }
            Op::TForLoop => {
                // TForLoop reads R[A+4] (control
                // returned by the iterator) and writes R[A+2] on
                // continue. R[A+4] must fit in the trace's frame.
                if a + 4 >= max_stack {
                    return None;
                }
            }
            _ => unreachable!("for_loop_idx_opt only set for Op::ForLoop / Op::TForLoop"),
        }
    }

    // `module` arrives as `&mut M` from the
    // caller. The JIT wrapper [`try_compile_trace_with_options`]
    // constructs a `JITModule` via [`build_trace_jit_module`]; the AOT
    // pipeline (luna-aot) feeds an `ObjectModule` of its own. The
    // helper-symbol contract is identical (both resolve `luna_jit_*` —
    // the JIT via `JITBuilder::symbol`, the AOT via static link).

    // Helper signatures — declared up front so emit can look them
    // up without re-declaring per call site. Unused declarations
    // get tree-shaken at optimization.
    let mut new_table_sig = module.make_signature();
    new_table_sig.returns.push(AbiParam::new(types::I64));
    let new_table_id = module
        .declare_function("luna_jit_new_table", Linkage::Import, &new_table_sig)
        .ok()?;

    // `fn luna_jit_table_set_{int,field}_checked(t, key, val_raw, val_tag)
    // -> stored` and `fn luna_jit_table_set_checked(t, key_raw, key_tag,
    // val_raw, val_tag) -> stored`
    let mut set_sig = module.make_signature();
    for _ in 0..4 {
        set_sig.params.push(AbiParam::new(types::I64));
    }
    set_sig.returns.push(AbiParam::new(types::I64));
    let mut set_any_sig = set_sig.clone();
    set_any_sig.params.push(AbiParam::new(types::I64));
    let set_ids = StoreHelpers {
        int_key: module
            .declare_function("luna_jit_table_set_int_checked", Linkage::Import, &set_sig)
            .ok()?,
        str_key: module
            .declare_function(
                "luna_jit_table_set_field_checked",
                Linkage::Import,
                &set_sig,
            )
            .ok()?,
        any_key: module
            .declare_function("luna_jit_table_set_checked", Linkage::Import, &set_any_sig)
            .ok()?,
    };

    // `fn luna_jit_table_get_field(t, key_ptr) -> raw`.
    let mut get_field_sig = module.make_signature();
    get_field_sig.params.push(AbiParam::new(types::I64));
    get_field_sig.params.push(AbiParam::new(types::I64));
    get_field_sig.returns.push(AbiParam::new(types::I64));
    let get_field_id = module
        .declare_function("luna_jit_table_get_field", Linkage::Import, &get_field_sig)
        .ok()?;

    // `fn luna_jit_op_get_tab_up(upval_idx, key_ptr) -> raw`.
    let mut get_tab_up_sig = module.make_signature();
    get_tab_up_sig.params.push(AbiParam::new(types::I64));
    get_tab_up_sig.params.push(AbiParam::new(types::I64));
    get_tab_up_sig.returns.push(AbiParam::new(types::I64));
    let get_tab_up_id = module
        .declare_function("luna_jit_op_get_tab_up", Linkage::Import, &get_tab_up_sig)
        .ok()?;

    // Checked table reads (`luna_jit_table_get_int_checked` et al.):
    // `fn(table_or_upval, key, want_tag, out: *mut i64) -> ok`.
    let mut get_checked_sig = module.make_signature();
    for _ in 0..4 {
        get_checked_sig.params.push(AbiParam::new(types::I64));
    }
    get_checked_sig.returns.push(AbiParam::new(types::I64));
    let get_int_checked_id = module
        .declare_function(
            "luna_jit_table_get_int_checked",
            Linkage::Import,
            &get_checked_sig,
        )
        .ok()?;
    let get_field_checked_id = module
        .declare_function(
            "luna_jit_table_get_field_checked",
            Linkage::Import,
            &get_checked_sig,
        )
        .ok()?;
    let get_tab_up_checked_id = module
        .declare_function(
            "luna_jit_op_get_tab_up_checked",
            Linkage::Import,
            &get_checked_sig,
        )
        .ok()?;

    // `fn luna_jit_op_closure(proto_idx: i64) -> i64`.
    // Returns the new Gc<LuaClosure> raw payload bits.
    let mut op_closure_sig = module.make_signature();
    op_closure_sig.params.push(AbiParam::new(types::I64));
    op_closure_sig.returns.push(AbiParam::new(types::I64));
    let op_closure_id = module
        .declare_function("luna_jit_op_closure", Linkage::Import, &op_closure_sig)
        .ok()?;

    // `fn luna_jit_spill_to_stack(slot_offset, tag, raw_bits)`.
    // Writes vm.stack[base + slot_offset] = Value::pack(tag, raw).
    let mut spill_sig = module.make_signature();
    spill_sig.params.push(AbiParam::new(types::I64));
    spill_sig.params.push(AbiParam::new(types::I64));
    spill_sig.params.push(AbiParam::new(types::I64));
    let spill_id = module
        .declare_function("luna_jit_spill_to_stack", Linkage::Import, &spill_sig)
        .ok()?;

    // `fn luna_jit_op_close(start_offset: i64) -> i64`.
    // Returns 0 (continue) or 1 (deopt — handler would run or
    // pre-existing pending_err).
    let mut op_close_sig = module.make_signature();
    op_close_sig.params.push(AbiParam::new(types::I64));
    op_close_sig.returns.push(AbiParam::new(types::I64));
    let op_close_id = module
        .declare_function("luna_jit_op_close", Linkage::Import, &op_close_sig)
        .ok()?;

    // `fn luna_jit_op_tforcall(abs_offset, nvars,
    // ctrl_out: *mut i64, key_out: *mut i64, val_out: *mut i64) -> i64`.
    // Batched: helper fills the three out pointers with raw bits
    // of R[A+2] / R[A+4] / R[A+5] and returns R[A+4]'s tag byte
    // (0..=11) on success, -1 on deopt. Emit reads the buffer via
    // cranelift `stack_load` IR (skips per-slot `stack_load` /
    // `stack_tag` helper calls — 4 helpers per iter would be the
    // bottleneck).
    let mut op_tforcall_sig = module.make_signature();
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.returns.push(AbiParam::new(types::I64));
    let op_tforcall_id = module
        .declare_function("luna_jit_op_tforcall", Linkage::Import, &op_tforcall_sig)
        .ok()?;

    // `fn luna_jit_stack_load(slot) -> i64` returns
    // raw bits of vm.stack[trace_head_frame.base + slot]. Used to
    // reload trace IR Variables after TForCall mutates vm.stack.
    let mut stack_load_sig = module.make_signature();
    stack_load_sig.params.push(AbiParam::new(types::I64));
    stack_load_sig.returns.push(AbiParam::new(types::I64));
    let stack_load_id = module
        .declare_function("luna_jit_stack_load", Linkage::Import, &stack_load_sig)
        .ok()?;

    // `fn luna_jit_stack_tag(slot) -> i64` returns
    // the raw::* tag byte of vm.stack[trace_head_frame.base + slot].
    // TForLoop tail emit dispatches on this to pick exit-on-Nil /
    // continue-on-Int / deopt-on-other.
    let mut stack_tag_sig = module.make_signature();
    stack_tag_sig.params.push(AbiParam::new(types::I64));
    stack_tag_sig.returns.push(AbiParam::new(types::I64));
    let stack_tag_id = module
        .declare_function("luna_jit_stack_tag", Linkage::Import, &stack_tag_sig)
        .ok()?;

    // `fn luna_jit_op_concat(a, n) -> i64`. Returns
    // 0 on success (result at vm.stack[base+a]) or -1 on deopt
    // (metamethod path, type error, length overflow,
    // pre-existing pending_err).
    let mut op_concat_sig = module.make_signature();
    op_concat_sig.params.push(AbiParam::new(types::I64));
    op_concat_sig.params.push(AbiParam::new(types::I64));
    op_concat_sig.returns.push(AbiParam::new(types::I64));
    let op_concat_id = module
        .declare_function("luna_jit_op_concat", Linkage::Import, &op_concat_sig)
        .ok()?;

    // `fn luna_jit_str_buf_acquire() -> i64`.
    // Returns a `*mut Vec<u8>` (boxed-leaked); used by buffered
    // accumulator emit at trace fn entry.
    let mut str_buf_acquire_sig = module.make_signature();
    str_buf_acquire_sig.returns.push(AbiParam::new(types::I64));
    let str_buf_acquire_id = module
        .declare_function(
            "luna_jit_str_buf_acquire",
            Linkage::Import,
            &str_buf_acquire_sig,
        )
        .ok()?;

    // `fn luna_jit_str_buf_release(buf: i64)`.
    let mut str_buf_release_sig = module.make_signature();
    str_buf_release_sig.params.push(AbiParam::new(types::I64));
    let str_buf_release_id = module
        .declare_function(
            "luna_jit_str_buf_release",
            Linkage::Import,
            &str_buf_release_sig,
        )
        .ok()?;

    // `fn luna_jit_str_buf_extend(buf, str_ptr) -> i64`.
    let mut str_buf_extend_sig = module.make_signature();
    str_buf_extend_sig.params.push(AbiParam::new(types::I64));
    str_buf_extend_sig.params.push(AbiParam::new(types::I64));
    str_buf_extend_sig.returns.push(AbiParam::new(types::I64));
    let str_buf_extend_id = module
        .declare_function(
            "luna_jit_str_buf_extend",
            Linkage::Import,
            &str_buf_extend_sig,
        )
        .ok()?;

    // `fn luna_jit_str_buf_intern(buf) -> i64`.
    let mut str_buf_intern_sig = module.make_signature();
    str_buf_intern_sig.params.push(AbiParam::new(types::I64));
    str_buf_intern_sig.returns.push(AbiParam::new(types::I64));
    let str_buf_intern_id = module
        .declare_function(
            "luna_jit_str_buf_intern",
            Linkage::Import,
            &str_buf_intern_sig,
        )
        .ok()?;
    // Squelch unused warnings.
    let _ = (
        str_buf_acquire_id,
        str_buf_release_id,
        str_buf_extend_id,
        str_buf_intern_id,
    );

    // `fn luna_jit_stack_update_raw(slot, raw)`.
    // Used in Op::Concat operand spill for Unset-kind slots.
    let mut update_raw_sig = module.make_signature();
    update_raw_sig.params.push(AbiParam::new(types::I64));
    update_raw_sig.params.push(AbiParam::new(types::I64));
    let update_raw_id = module
        .declare_function(
            "luna_jit_stack_update_raw",
            Linkage::Import,
            &update_raw_sig,
        )
        .ok()?;

    let mut get_int_sig = module.make_signature();
    get_int_sig.params.push(AbiParam::new(types::I64));
    get_int_sig.params.push(AbiParam::new(types::I64));
    get_int_sig.returns.push(AbiParam::new(types::I64));
    let get_int_id = module
        .declare_function("luna_jit_table_get_int", Linkage::Import, &get_int_sig)
        .ok()?;

    let suppress_admit_id = module
        .declare_function(
            "luna_jit_suppress_trace_admit",
            Linkage::Import,
            &module.make_signature(),
        )
        .ok()?;
    let mut math_fn_check_sig = module.make_signature();
    math_fn_check_sig.params.push(AbiParam::new(types::I64));
    math_fn_check_sig.params.push(AbiParam::new(types::I64));
    math_fn_check_sig.returns.push(AbiParam::new(types::I64));
    let math_fn_check_id = module
        .declare_function(
            "luna_jit_math_fn_is_library",
            Linkage::Import,
            &math_fn_check_sig,
        )
        .ok()?;

    let mut len_sig = module.make_signature();
    len_sig.params.push(AbiParam::new(types::I64));
    len_sig.returns.push(AbiParam::new(types::I64));
    let len_checked_id = module
        .declare_function("luna_jit_table_len_checked", Linkage::Import, &len_sig)
        .ok()?;

    // `fn luna_jit_upval_get(idx: i64) -> i64`. The
    // helper reads `JIT_CL`'s upvals[idx], unpacks to raw payload,
    // returns it as i64. Type tag is lost across the ABI; the
    // dispatcher's exit_tags must use the Untouched fallback
    // (carry the entry tag through) since we can't statically
    // determine what kind of Value an upval holds.
    let mut upval_get_sig = module.make_signature();
    upval_get_sig.params.push(AbiParam::new(types::I64));
    upval_get_sig.returns.push(AbiParam::new(types::I64));
    let upval_get_id = module
        .declare_function("luna_jit_upval_get", Linkage::Import, &upval_get_sig)
        .ok()?;
    let mut head_closure_sig = module.make_signature();
    head_closure_sig.returns.push(AbiParam::new(types::I64));
    let head_closure_id = module
        .declare_function("luna_jit_head_closure", Linkage::Import, &head_closure_sig)
        .ok()?;

    // `fn luna_jit_trace_materialize_frames(n: u64,
    // metas: *const FrameMaterializeInfo) -> i64`. Called by the
    // lowerer's cmp@d>0 emit.
    let mut materialize_sig = module.make_signature();
    materialize_sig.params.push(AbiParam::new(types::I64));
    materialize_sig.params.push(AbiParam::new(types::I64));
    materialize_sig.returns.push(AbiParam::new(types::I64));
    let materialize_id = module
        .declare_function(
            "luna_jit_trace_materialize_frames",
            Linkage::Import,
            &materialize_sig,
        )
        .ok()?;

    // `fn luna_jit_materialize_sunk_table(cap: i64,
    // raws_ptr: *const u64, kinds_ptr: *const u8) -> i64`. Emit
    // per cmp side-exit per live Sinkable site: stack-allocates
    // a `cap × 8` raws buffer + a `cap × 1` kinds buffer, fills
    // them from the site's virt slot Variables + virt_kinds tracker,
    // calls this helper, writes the returned `Value::Table` raw
    // bits into the slot's regs Variable so the subsequent
    // `store_back` lands the heap pointer in `reg_state[a]`.
    // 7 i64 args:
    //   cap, arr_raws, arr_kinds, n_hash, hash_keys, hash_raws, hash_kinds
    // Returns: heap table raw payload (i64 Gc<Table> ptr).
    let mut mat_sunk_sig = module.make_signature();
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.returns.push(AbiParam::new(types::I64));
    let mat_sunk_id = module
        .declare_function(
            "luna_jit_materialize_sunk_table",
            Linkage::Import,
            &mat_sunk_sig,
        )
        .ok()?;

    let mut sig = module.make_signature();
    // Param 0 — reg_state ptr (caller-owned, lives across the call).
    sig.params.push(AbiParam::new(types::I64));
    // Return — continuation PC (head_pc on clean close).
    sig.returns.push(AbiParam::new(types::I64));
    // caller-provided name +
    // export linkage when driving the AOT pipeline. The JIT wrapper
    // (`try_compile_trace_with_options`) passes `None`, preserving the
    // original `luna_jit_trace` / `Linkage::Local` shape.
    let (trace_fn_name, trace_fn_linkage) = match aot_fn_name {
        Some(name) => (name, Linkage::Export),
        None => ("luna_jit_trace", Linkage::Local),
    };
    let fn_id = module
        .declare_function(trace_fn_name, trace_fn_linkage, &sig)
        .ok()?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, fn_id.as_u32());

    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);

    // Two-block layout for the trace body:
    //
    // - `entry` is the function entry — receives the `reg_state`
    //   pointer as block param 0, loads each Lua reg from memory
    //   into a cranelift Variable, then unconditionally jumps to
    //   `body_loop`. The reg-load prelude runs *once* per
    //   dispatcher entry.
    // - `body_loop` is the loop head. The recorded op IR emits
    //   into it (or into successor blocks split off by cmp brifs).
    //   At the trace's clean close — when no `Op::Call` has
    //   truncated it — the tail emits a jump *back* to `body_loop`,
    //   so subsequent iterations stay inside the JIT'd code until
    //   a cmp side-exits. The dispatcher's per-iter marshal
    //   overhead amortizes across however many iterations the
    //   trace runs internally.
    //
    // Cranelift `FunctionBuilder` handles the back-edge phis
    // automatically: every reg's Variable gets a phi at
    // `body_loop`'s entry merging the entry-from-`entry` def
    // (initial load) with the loop-back def (the previous
    // iteration's writes). We delay sealing `body_loop` until
    // after the tail emits its back-edge so cranelift knows both
    // predecessors.
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    bcx.seal_block(entry);
    let reg_state = bcx.block_params(entry)[0];

    // import the `TraceFn` ABI signature once so
    // every side-exit emit can `call_indirect` into a child side
    // trace. Matches the parent's own signature (`(I64) -> I64`).
    let trace_fn_sig_ref: cranelift_codegen::ir::SigRef = {
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        bcx.func.import_signature(sig)
    };
    // singleton GLOBAL side-trace cell shared by
    // every non-INLINE / non-TAG callsite (clean-tail, Call
    // truncation, ForLoop / TForLoop exits, generic deopts). Each
    // such callsite bakes this Box's heap address into its IR.
    // Transported into [`CompiledTrace::global_side_trace_ptr`] at
    // emit end without moving (Box's heap allocation stays put).
    let global_side_trace_box: Box<TCellPtr> = Box::new(TCellPtr::null());
    let _global_side_trace_cell_addr = (&*global_side_trace_box) as *const TCellPtr as i64;

    // `regs_full` is sized to `window_size_us`, big
    // enough for every inlined frame's register window. Slots
    // [0..max_stack) are loaded from reg_state (caller-marshalled);
    // [max_stack..window_size_us) start as `iconst(0)` so the
    // callee's `GetUpval` / arith fills them. The emit loop below
    // shadows `regs` to the per-op window slice so existing
    // `regs[ins.X()]` indexing automatically shifts across inlined
    // frames without rewriting every access.
    let mut regs_full: Vec<Variable> = Vec::with_capacity(window_size_us);
    for i in 0..window_size_us {
        let v = bcx.declare_var(types::I64);
        if i < max_stack {
            let offset = (i as i32) * 8;
            let v0 = bcx
                .ins()
                .load(types::I64, MemFlagsData::new(), reg_state, offset);
            bcx.def_var(v, v0);
        } else {
            let z = bcx.ins().iconst(types::I64, 0);
            bcx.def_var(v, z);
            // Exits store only what changed since (see `sync_reg_state`),
            // so reg_state must hold the zero too: a side trace entered
            // from its parent's exit finds the parent's values here.
            bcx.ins()
                .store(MemFlagsData::new(), z, reg_state, (i as i32) * 8);
        }
        regs_full.push(v);
    }
    // Variable carrying R[A+4]'s tag byte across the
    // TForCall body emit → TForLoop tail emit boundary. TForCall's
    // batched helper returns the tag on success; tail emit reads
    // it via use_var to dispatch on Nil / Int / other instead of
    // calling the `luna_jit_stack_tag` helper. Declared
    // unconditionally — only def_var'd if the trace actually has a
    // TForCall (otherwise unused, cranelift tree-shakes).
    let tforcall_tag_var = bcx.declare_var(types::I64);
    // The tag of the value TForCall produced (R[A+5]), for the TForLoop
    // back-edge check.
    let tforcall_val_tag_var = bcx.declare_var(types::I64);
    {
        let z = bcx.ins().iconst(types::I64, 0);
        bcx.def_var(tforcall_tag_var, z);
        bcx.def_var(tforcall_val_tag_var, z);
    }

    // depth-relative `base_var` scaffold.
    //
    // The Variable is declared at trace head (here, in the entry
    // block immediately after the reg_state load prelude) and
    // initialised to `iconst(0)` as the depth-0 sentinel
    // placeholder. No op-arm reads it yet; they still index
    // `regs_full[off + slot]`.
    //
    // An unused Variable initialized via a single iconst gets DCE'd
    // by Cranelift's mid-end, so the scaffold is overhead-neutral.
    //
    // Probe: `BASE_VAR_SCAFFOLD_DECLARED` bumps exactly once at the
    // post-def_var point so the regression test
    // `base_var_scaffold.rs` can assert "scaffold ran" on
    // an arbitrary fixture trace without scraping IR text. Bump
    // happens after `def_var` so a `declare_var` panic earlier leaves
    // the counter unchanged.
    let base_var = bcx.declare_var(types::I64);
    {
        let z = bcx.ins().iconst(types::I64, 0);
        bcx.def_var(base_var, z);
        // Mirror the tforcall_tag_var declaration pattern exactly
        // (declare + iconst init + def_var, no anchor use). Cranelift
        // tree-shakes the unused Variable in optimized builds, so the
        // scaffold adds zero machine-code residue.
        BASE_VAR_SCAFFOLD_DECLARED.with(|c| c.set(c.get().wrapping_add(1)));
    }

    // allocate virtual `Variable`s for each Sinkable
    // site that meets the sunk-emit criteria. Sites that don't
    // meet the criteria are demoted to Escaped right here so the
    // body emit's site-state check naturally falls through to the
    // existing heap-alloc helper path. Criteria:
    //   - `inline_depth == 0` (trace head's frame only — inline
    //     sinking requires extra plumbing for
    //     the materialize helper to address inlined windows)
    //   - `array_cap` in `1..=MAX_SUNK_CAP` (cap = 0 means the
    //     site didn't decode an array part; cap > MAX is a
    //     Cranelift Variable budget guard)
    //   - the site's slot is NOT the trace-terminator `Op::Return1`
    //     R[A] — sinking that case needs the materialize helper
    //     to repack the array into a heap `Gc<Table>` on the way
    //     out
    //   - the trace's body has NO cmp ops (`Lt`/`Le`/`Eq`/`EqK`) —
    //     a cmp emits a side-exit and the interp resume needs the
    //     heap table; the sweep escapes all live bindings on
    //     a cmp, but we ALSO need to bail on body cmps that fire
    //     AFTER the site dies (no live binding to escape, but the
    //     trace still has a back-edge candidate).
    //
    // Note: looping traces (`opts.internal_loop = true`) that have
    // any cmp in body are already excluded by the sweep escape
    // rule. ForLoop terminators escape via the terminator rule
    // (TraceEnd::ForLoop → all live). So we don't need an explicit
    // `internal_loop` check here.
    const MAX_SUNK_CAP: u32 = 8;
    let return_a_for_sunk_check: Option<u32> = match end_idx_opt {
        Some((idx, TraceEnd::Return)) if idx < record.ops.len() => {
            let term = &record.ops[idx];
            if matches!(term.inst.op(), Op::Return1) && term.inline_depth == 0 {
                Some(term.inst.a())
            } else {
                None
            }
        }
        _ => None,
    };
    // There is no inline-cmp gate: inline cmp
    // side-exits (per_exit_inline arm) call
    // `emit_materialize_live_sunk` to reconstruct live sunk sites
    // before the frame-mat helper pushes inline frames, so a
    // depth>0 cmp doesn't demote sites.
    let mut virt_vars: Vec<Option<Vec<Variable>>> = vec![None; escape.sites.len()];
    let mut virt_kinds: Vec<Option<Vec<RegKind>>> = vec![None; escape.sites.len()];
    let mut sunk_alloc_seen: u32 = 0;
    // incremented at each cmp side-exit emit point that
    // materialises ≥1 live Sinkable site. Telemetry only; the
    // dispatcher's runtime materialise calls are not counted here
    // (this is a per-trace static count of emit sites that emit
    // the helper call).
    let mut materialize_emit_count: u32 = 0;
    let mut closure_seen: u32 = 0;
    for (idx, site) in escape.sites.iter_mut().enumerate() {
        if site.state != EscapeState::Sinkable {
            continue;
        }
        // depth>0 sites are sunk-eligible. Materialise
        // (`emit_materialize_live_sunk`) handles BOTH depth=0 and
        // depth>0 sites at depth=0 cmp arm AND inline cmp
        // (per_exit_inline) arm, since inline cmp side-exits
        // reconstruct live sunk sites. `return_a` check only matters for depth=0
        // (TraceEnd::Return applies at the trace-head frame).
        // total virt slot count = array_cap + hash_keys.
        // - array-only site:    cap = array_cap,           hash = 0
        // - hash-only site:     cap = 0,                   hash = hash_keys.len()
        // - mixed array+hash:   cap = array_cap > 0,       hash > 0
        // - empty (no ops):     cap = 0,                   hash = 0 → demoted below
        let array_cap = site.array_cap as usize;
        let n_hash = site.hash_keys.len();
        let total_slots = array_cap + n_hash;
        if total_slots == 0
            || array_cap > MAX_SUNK_CAP as usize
            || (site.inline_depth == 0 && return_a_for_sunk_check == Some(site.a))
        {
            site.state = EscapeState::Escaped;
            continue;
        }
        // hash slot materialise is plumbed into
        // emit_materialize_live_sunk (extended helper signature
        // carries hash_keys + hash_raws + hash_kinds buffers), so no
        // has_any_cmp gate is needed. Hash sites survive cmp side-exits via
        // table.set(Value::Str(key), ...) at materialise time.
        let mut vars = Vec::with_capacity(total_slots);
        for _ in 0..total_slots {
            let v = bcx.declare_var(types::I64);
            let z = bcx.ins().iconst(types::I64, 0);
            bcx.def_var(v, z);
            vars.push(v);
        }
        virt_vars[idx] = Some(vars);
        virt_kinds[idx] = Some(vec![RegKind::Unset; total_slots]);
        sunk_alloc_seen += 1;
    }

    // if an active_accum is in play,
    // declare buf_var, emit acquire IR, and populate flush_ctx
    // with Some(FlushCtx { ... }). All 19 existing
    // emit_store_back_and_return_* call sites then auto-flush
    // (intern → def_var(accum_slot) → release) before storing
    // back to reg_state.
    if let Some(ref ba) = active_accum {
        let buf_var = bcx.declare_var(types::I64);
        let acquire_ref = module.declare_func_in_func(str_buf_acquire_id, bcx.func);
        let intern_ref = module.declare_func_in_func(str_buf_intern_id, bcx.func);
        let release_ref = module.declare_func_in_func(str_buf_release_id, bcx.func);
        let extend_ref = module.declare_func_in_func(str_buf_extend_id, bcx.func);
        let call_inst = bcx.ins().call(acquire_ref, &[]);
        let ptr = bcx.inst_results(call_inst)[0];
        bcx.def_var(buf_var, ptr);
        // prepend the accumulator slot's current
        // bytes into the buffer. The dispatcher always fires on
        // iter 2+ (interp's TForLoop trigger fires AFTER iter 1's
        // body has run), so by the time the trace fn entry
        // executes, `R[accum_slot]` already holds the result of
        // `s` after iter 1 (= entry_s_initial + piece_1). Without
        // this prepend, the flush at exit produces only iter 2..N
        // bytes; the test workload `s = '[' .. iter1 .. ...` loses
        // the leading `[piece_1`. Net effect: buf = accum_slot's
        // entry bytes + iter 2..N piece bytes; flush intern's all
        // bytes; correct result.
        let accum_raw = bcx.use_var(regs_full[ba.accum_slot as usize]);
        let buf_ptr = bcx.use_var(buf_var);
        let _ = bcx.ins().call(extend_ref, &[buf_ptr, accum_raw]);
        flush_ctx = Some(FlushCtx {
            buf_var,
            accum_slot: ba.accum_slot,
            intern_ref,
            release_ref,
        });
    }

    // Nothing in the trace can reassign `math.<fn>` unless it stores a
    // field of that name or `math` itself, or stores under a key it does
    // not know (SetTable); calls end the trace and the table helpers
    // deopt on `__newindex`. Without such a store the math folds are
    // checked once, in `precheck` before the loop head, rather than on
    // every iteration.
    let fold_check_once = !record.ops[..effective_end].iter().any(|rop| {
        let key = |k: u32| match head_proto.consts.get(k as usize) {
            Some(luna_core::runtime::Value::Str(s)) => Some(s.as_bytes()),
            _ => None,
        };
        match rop.inst.op() {
            Op::SetTable => true,
            // the key is K[B] for both
            Op::SetField | Op::SetTabUp => match key(rop.inst.b()) {
                Some(name) => {
                    name == b"math" || math_folds.iter().any(|f| f.fn_name.as_bytes() == name)
                }
                None => true,
            },
            _ => false,
        }
    });
    // Filled in below, once the exit bookkeeping exists.
    let precheck = (fold_check_once && !math_folds.is_empty()).then(|| bcx.create_block());

    let body_loop = bcx.create_block();
    bcx.ins().jump(precheck.unwrap_or(body_loop), &[]);
    // `body_loop` is entered after the precheck block is emitted (below):
    // reading a register there first would leave it half-built while
    // another block is emitted, which the builder rejects.

    // What reg_state holds for each register at the loop head: on entry
    // the values the prelude loaded (caller window) or the zeroes the
    // dispatcher filled it with (inline frames); on the back-edge what
    // `sync_reg_state` wrote before the jump. Read at the loop head below.
    let mut stored: Vec<Option<Value>> = Vec::new();

    // Per-reg current kind: the recorded entry tag for a head-frame
    // register the trace reads before writing (the dispatcher checks
    // it), held on the stack for the other head-frame registers, and
    // Unset past the head frame (the dispatcher zero-initialises those
    // reg_state slots and trace IR fills them via writers). Sized to
    // `window_size_us` (mirrors `regs_full`).
    let mut current_kinds: Vec<RegKind> = (0..window_size_us)
        .map(|i| match head_live.get(i) {
            Some(true) => record
                .entry_tags
                .get(i)
                .and_then(|&t| RegKind::from_entry_tag(t))
                .unwrap_or(RegKind::Unset),
            Some(false) => RegKind::StackHeld,
            None => RegKind::Unset,
        })
        .collect();
    // The kinds the body is lowered for. A back-edge may run it again
    // only when the caller window holds these same kinds (see
    // `loop_kinds_match`); otherwise
    // the next pass would read (and hand to an exit) a register with
    // bits of one kind as another, e.g. an Int as a Float.
    let head_kinds: Vec<RegKind> = current_kinds[..max_stack].to_vec();
    let mut dispatchable: bool = true;
    // the first emit-pass site that flips
    // dispatchable to false wins this label; CompiledTrace
    // exposes it via `dispatch_off_reason` for probe diagnostics.
    let mut dispatch_off_reason: Option<&'static str> = None;
    // per-side-exit RegKind snapshot. Pushed at each
    // true side-exit emit site (Lt/Le/Eq + Jmp) so later writers
    // (e.g. `Op::GetUpval` whose result we infer as `Closure`) don't
    // pollute the side-exit's restore with a tag the slot hasn't
    // actually become at that exit. The clean-tail and call-truncation
    // paths reuse the final `current_kinds` via `ct.exit_tags`.
    // 3rd element is the per-entry `Box<Cell<*const
    // u8>>` whose heap address is baked into the corresponding
    // emit_store_back_and_return_pc callsite. Allocated at each push
    // site BEFORE the helper call so the IR's `iconst`-baked address
    // exists. Transported through into `tags_side_trace_ptrs` at the
    // end of emit (the cell never moves).
    let mut per_exit_kinds: Vec<(u32, Vec<RegKind>, Box<TCellPtr>)> = Vec::new();
    // per inline cmp@d>0 side-exit. Each entry
    // is built at the cmp emit site and includes the side-exit PC,
    // a window-sized exit-tag snapshot, and the frame-mat chain. The
    // IR encodes `(site_idx + 1)` in the upper 32 bits of the
    // returned i64 so the dispatcher can pick the right entry
    // without colliding on shared cont_pc values (fib's cmp@d=0
    // through cmp@d=4 all side-exit to the same PC).
    // 5th element is the per-site `Box<Cell<*const
    // u8>>` whose heap address is baked into the IR's
    // `emit_store_back_and_return_site` gate. Allocated at each push
    // site BEFORE the helper call (address is
    // stable across `Vec → Rc<[]>` moves because Box transfers
    // ownership without moving the heap cell).
    let mut per_exit_inline_vec: Vec<(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )> = Vec::new();
    // Live call stack mirror — push on self-recursive `Op::Call`,
    // pop on `Op::Return0/1` at depth>0. Each frame's `base_offset`
    // and `pc` (= caller's Call.pc + 1) are stamped at push time;
    // when snapshotting at a cmp@d>0 site, the innermost frame's
    // `pc` is overwritten with the actual side-exit PC so the helper
    // pushes the right resume point without needing a dispatcher
    // post-hoc fix-up.
    let mut call_chain: Vec<FrameMaterializeInfo> = Vec::new();

    // --- emit body
    //
    // Cranelift's `FunctionBuilder` tracks the "current" block
    // internally; every `bcx.ins()` emits into whichever block was
    // last `switch_to_block`'d. A cmp's `brif` forks the current
    // block to a `continue_blk` and a `side_exit_blk`; after
    // emitting the side-exit and switching back to `continue_blk`,
    // subsequent ops land there. By the end of the loop the
    // "current" block is whatever the last cmp's continue branch
    // pointed at (or the entry block if no cmps fired).
    //
    // Only the *normal* range (`record.ops[..effective_end]`) is
    // emitted. If `Op::Call` truncates the trace, the tail emits
    // a side-exit at the Call's PC instead of the head_pc close.
    // Memoize GetUpval(idx) per dispatch.
    // For self-recursive traces (fib, factorial, etc.), the trace head
    // is entered with one closure and `JIT_CL` stays pinned to it for
    // the entire dispatch; all inlined-depth GetUpval(idx) calls return
    // the same value. Hoist the helper call to the first occurrence and
    // reuse the cached SSA value at later sites (in cranelift-dominated
    // blocks). For fib's 3-deep inline trace, this cuts 4 helper calls
    // to 1 per dispatch (~60 cycles saved × 163k dispatches ≈ 3-5 ms,
    // ~10-15% win).
    //
    // SAFETY of memoization:
    // - Cache invalidation: none required within a single trace
    //   dispatch — `JIT_CL` is pinned at entry and unchanged through
    //   the entire trace body. The first GetUpval(idx) call materializes
    //   the value; subsequent reads of the same idx are exact duplicates.
    // - Cross-block validity: cached values are stored in a Variable
    //   (via def_var / use_var); cranelift's FunctionBuilder inserts
    //   phis as needed for cross-block reads.
    // - Side-exit safety: the first occurrence may be in a block reached
    //   only on the recursive path (e.g. block2 in fib). If a side-exit
    //   fires BEFORE that block (e.g. head-fail base case in block3),
    //   the cache is never populated and reuse never happens — correct.
    let mut upval_cache: std::collections::HashMap<u32, Variable> =
        std::collections::HashMap::new();
    // the entry closure, fetched at the first inlined call; the trace
    // is linear, so that fetch dominates every later call
    let mut head_closure_var: Option<Variable> = None;
    // No iconst memoization: the arm64 backend folds
    // `iconst+isub`/`iconst+icmp` into immediate-form instructions
    // at codegen, so it would add little.
    // A guard that fails leaves the trace at `$pc` (the op being
    // guarded, re-executed by the interpreter) exactly as a cmp side
    // exit does: live sunk tables are materialised and, when the op sits
    // in an inlined frame, the frames are rebuilt first. An exit to the
    // head also stops the dispatcher from entering the trace again before
    // the interpreter has run the head op (see `emit_tagged_exit`).
    macro_rules! guard_exit {
        ($pc:expr, $i:expr) => {{
            let side_exit_pc: u32 = $pc;
            if !call_chain.is_empty() {
                let head_resume_pc = call_chain[0].pc;
                let mut snapshot: Vec<FrameMaterializeInfo> = call_chain.clone();
                if let Some(last) = snapshot.last_mut() {
                    last.pc = side_exit_pc;
                }
                let chain_rc: TArc<[FrameMaterializeInfo]> = snapshot.into();
                let chain_ptr = TArc::as_ptr(&chain_rc) as *const FrameMaterializeInfo as i64;
                let chain_len = chain_rc.len() as i64;
                let site_idx = per_exit_inline_vec.len() as u32;
                let mut kinds_snapshot: Vec<RegKind> = current_kinds.clone();
                let mat_count = emit_materialize_live_sunk(
                    &mut bcx,
                    &mut module,
                    mat_sunk_id,
                    &escape,
                    &virt_vars,
                    &virt_kinds,
                    &regs_full,
                    &op_offsets,
                    $i,
                    &mut kinds_snapshot,
                    head_proto,
                    opts.aot,
                    &mut defined_aot_data,
                );
                materialize_emit_count += mat_count;
                let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
                let chain_for_helper = chain_rc.clone();
                per_exit_inline_vec.push((
                    side_exit_pc,
                    head_resume_pc,
                    kinds_snapshot,
                    chain_rc,
                    side_box,
                ));
                let n_arg = bcx.ins().iconst(types::I64, chain_len);
                let ptr_arg = emit_chain_ptr_arg(
                    &mut module,
                    &mut bcx,
                    &chain_for_helper,
                    chain_ptr,
                    opts.aot,
                    &mut defined_aot_data,
                );
                let mat_ref = module.declare_func_in_func(materialize_id, bcx.func);
                let _ = bcx.ins().call(mat_ref, &[n_arg, ptr_arg]);
                emit_store_back_and_return_site(
                    &mut bcx,
                    &regs_full[..window_size_us],
                    &stored,
                    reg_state,
                    site_idx,
                    side_exit_pc,
                    flush_ctx.as_ref(),
                    0i64,
                    trace_fn_sig_ref,
                );
            } else {
                let mut snapshot: Vec<RegKind> = current_kinds[..max_stack].to_vec();
                let mat_count = emit_materialize_live_sunk(
                    &mut bcx,
                    &mut module,
                    mat_sunk_id,
                    &escape,
                    &virt_vars,
                    &virt_kinds,
                    &regs_full,
                    &op_offsets,
                    $i,
                    &mut snapshot,
                    head_proto,
                    opts.aot,
                    &mut defined_aot_data,
                );
                materialize_emit_count += mat_count;
                let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
                let tag_side_local = per_exit_kinds.len() as u32;
                per_exit_kinds.push((side_exit_pc, snapshot, side_box));
                emit_tagged_exit(
                    &mut bcx,
                    &mut module,
                    suppress_admit_id,
                    &regs_full[..max_stack],
                    &stored,
                    reg_state,
                    side_exit_pc,
                    record.head_pc,
                    tag_side_local,
                    flush_ctx.as_ref(),
                    trace_fn_sig_ref,
                );
            }
        }};
    }
    // Call a checked read helper; on failure leave the trace at `$pc`,
    // otherwise evaluate to the payload it wrote.
    macro_rules! checked_read {
        ($id:expr, $a0:expr, $a1:expr, $want:expr, $pc:expr, $i:expr) => {{
            let out_ss = bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                8,
                3,
            ));
            let out_addr = bcx.ins().stack_addr(types::I64, out_ss, 0);
            let want = bcx.ins().iconst(types::I64, $want as i64);
            let fref = module.declare_func_in_func($id, bcx.func);
            let call = bcx.ins().call(fref, &[$a0, $a1, want, out_addr]);
            let ok = bcx.inst_results(call)[0];
            let cont_blk = bcx.create_block();
            let exit_blk = bcx.create_block();
            bcx.ins().brif(ok, cont_blk, &[], exit_blk, &[]);
            bcx.switch_to_block(exit_blk);
            bcx.seal_block(exit_blk);
            guard_exit!($pc, $i);
            bcx.switch_to_block(cont_blk);
            bcx.seal_block(cont_blk);
            bcx.ins().stack_load(types::I64, types::I64, out_ss, 0)
        }};
    }
    // Continue in a new block when `$cond` holds, else take a
    // `guard_exit!` to `$pc`.
    macro_rules! guard {
        ($cond:expr, $i:expr, $pc:expr) => {{
            let continue_blk = bcx.create_block();
            let exit_blk = bcx.create_block();
            bcx.ins().brif($cond, continue_blk, &[], exit_blk, &[]);
            bcx.switch_to_block(exit_blk);
            bcx.seal_block(exit_blk);
            guard_exit!($pc, $i);
            bcx.switch_to_block(continue_blk);
            bcx.seal_block(continue_blk);
        }};
    }
    // Integer constants the registers hold at this point of the trace
    // (from LoadI / LoadK earlier in the same pass), so a `//`, `%` or shift
    // by a constant needs no runtime guard.
    let mut known_int: Vec<Option<i64>> = vec![None; window_size_us];
    if let Some(precheck) = precheck {
        // Nothing has run yet: a failed check leaves at the head with the
        // entry kinds, and the interpreter makes the calls.
        bcx.switch_to_block(precheck);
        bcx.seal_block(precheck);
        // Before the loop head reg_state holds what the prelude loaded.
        let entry_stored: Vec<Option<Value>> =
            regs_full.iter().map(|&v| Some(bcx.use_var(v))).collect();
        // interned, so one pointer per name
        let mut checked: Vec<*const u8> = Vec::new();
        for fold in &math_folds {
            let math_key = head_proto.consts[record.ops[fold.start_idx].inst.c() as usize];
            let name_key = head_proto.consts[record.ops[fold.start_idx + 1].inst.c() as usize];
            let (
                luna_core::runtime::Value::Str(math_key),
                luna_core::runtime::Value::Str(name_key),
            ) = (math_key, name_key)
            else {
                unreachable!("the fold matcher took both keys as strings");
            };
            let name_ptr = name_key.as_ptr() as *const u8;
            if checked.contains(&name_ptr) {
                continue;
            }
            checked.push(name_ptr);
            let m = emit_str_key_arg(module, &mut bcx, math_key, opts.aot, &mut defined_aot_data);
            let k = emit_str_key_arg(module, &mut bcx, name_key, opts.aot, &mut defined_aot_data);
            let check_ref = module.declare_func_in_func(math_fn_check_id, bcx.func);
            let call = bcx.ins().call(check_ref, &[m, k]);
            let is_library = bcx.inst_results(call)[0];
            let ok_blk = bcx.create_block();
            let exit_blk = bcx.create_block();
            bcx.ins().brif(is_library, ok_blk, &[], exit_blk, &[]);
            bcx.switch_to_block(exit_blk);
            bcx.seal_block(exit_blk);
            let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
            let tags_idx = per_exit_kinds.len() as u32;
            per_exit_kinds.push((
                record.head_pc,
                current_kinds[..max_stack].to_vec(),
                side_box,
            ));
            emit_tagged_exit(
                &mut bcx,
                &mut module,
                suppress_admit_id,
                &regs_full[..max_stack],
                &entry_stored,
                reg_state,
                record.head_pc,
                record.head_pc,
                tags_idx,
                flush_ctx.as_ref(),
                trace_fn_sig_ref,
            );
            bcx.switch_to_block(ok_blk);
            bcx.seal_block(ok_blk);
        }
        bcx.ins().jump(body_loop, &[]);
    }
    bcx.switch_to_block(body_loop);
    // Intentionally NOT sealed: the tail's clean-close back-edge
    // adds a second predecessor below.
    stored.extend(regs_full.iter().map(|&v| Some(bcx.use_var(v))));
    // the virtual register of a constant-operand op (see `vconsts`)
    let kvar = bcx.declare_var(types::I64);
    checkpoint("pre:main-emit-loop");
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        // Commit the previous op's register writes to reg_state.
        sync_reg_state(&mut bcx, &regs_full, &mut stored, reg_state);
        let vk = vconst(i);
        // R[C] of a register-operand op, read before this op's own write
        // forgets it (`x = x % 7` divides by the old value)
        let rc_const = match vk {
            Some(k) if rop.inst.c() as usize == max_stack => match k {
                VConst::Int(n) => Some(n),
                VConst::Float(_) => None,
            },
            _ => known_int
                .get(op_offsets[i] as usize + rop.inst.c() as usize)
                .copied()
                .flatten(),
        };
        for w in op_writes_at_offset(rop, op_offsets[i]) {
            if let Some(slot) = known_int.get_mut(w as usize) {
                *slot = None;
            }
        }
        // `off` is the start of this op's register
        // window inside reg_state_buf. `regs` is shadowed to the
        // matching slice of `regs_full`, so existing `regs[ins.X()]`
        // indexing auto-shifts across inlined frames. `current_kinds`
        // is NOT shadowed (mut sub-slice would block Return1's
        // cross-window write) — emit code reads/writes via the full
        // Vec with explicit `off + X` indexing.
        let off = op_offsets[i] as usize;
        let regs: &[Variable] = &regs_full[off..off + max_stack];
        // a constant operand: its value in `kvar`, which `regs` gets as
        // register `max_stack`
        let regs_v: Vec<Variable>;
        let regs: &[Variable] = match vk {
            Some(k) => {
                let v = match k {
                    VConst::Int(n) => bcx.ins().iconst(types::I64, n),
                    VConst::Float(f) => {
                        let fv = bcx.ins().f64const(f);
                        bcx.ins().bitcast(types::I64, MemFlagsData::new(), fv)
                    }
                };
                bcx.def_var(kvar, v);
                regs_v = regs.iter().copied().chain([kvar]).collect();
                &regs_v
            }
            None => regs,
        };
        // the kind of an operand register, the virtual one included
        macro_rules! kind {
            ($r:expr) => {{
                let r: u32 = $r;
                match vk {
                    Some(VConst::Int(_)) if r as usize == max_stack => RegKind::Int,
                    Some(VConst::Float(_)) if r as usize == max_stack => RegKind::Float,
                    _ => k_op(&current_kinds, off as u32 + r),
                }
            }};
        }
        // body emit handler for the 4-op
        // string-accumulator idiom. Skip the 2 pre-Moves + the
        // post-Move (they're collapsed into the buffered emit).
        // Replace the Concat with `luna_jit_str_buf_extend(buf,
        // piece_raw)` + a deopt branch on -1 (piece wasn't Str
        // → existing __concat metamethod path takes over).
        if let Some(ref ba) = active_accum
            && let Some(ref fctx) = flush_ctx
        {
            if i == ba.pre1_idx || i == ba.pre2_idx || i == ba.post_idx {
                continue;
            }
            if i == ba.concat_idx {
                // Read piece slot raw bits + buf ptr.
                let piece_raw = bcx.use_var(regs[ba.piece_slot as usize]);
                let buf_ptr = bcx.use_var(fctx.buf_var);
                let extend_ref = module.declare_func_in_func(str_buf_extend_id, bcx.func);
                let call_inst = bcx.ins().call(extend_ref, &[buf_ptr, piece_raw]);
                let status = bcx.inst_results(call_inst)[0];
                // Branch on -1 (signed less than 0) → deopt.
                let zero = bcx.ins().iconst(types::I64, 0);
                let is_err = bcx.ins().icmp(IntCC::SignedLessThan, status, zero);
                let continue_blk = bcx.create_block();
                let deopt_blk = bcx.create_block();
                bcx.ins().brif(is_err, deopt_blk, &[], continue_blk, &[]);
                // Deopt path: flush buffer + store back + return pc.
                bcx.switch_to_block(deopt_blk);
                bcx.seal_block(deopt_blk);
                // restored with the kinds the registers have here
                guard_exit!(rop.pc, i);
                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
                continue;
            }
        }
        if consumed_by_cmp[i] {
            // The cmp at i-1 already accounted for this Jmp via
            // its `brif`'s continue edge; emitting jump IR here
            // would double-jump.
            continue;
        }
        // Math fold emit. Layout (see `math_folds` doc above):
        //
        //   * `Libm1` — folded indices are `start..=start+3`. The
        //     emit fires at `start` (single libm call); the trailing
        //     3 ops are silent.
        //
        //   * `Min2 / Max2` — folded indices are `start`, `start+1`,
        //     and `call_idx`. The emit fires at `call_idx` (the
        //     `Op::Call`) because that's when args have already
        //     been computed into R[A+1] / R[A+2] by the standard
        //     arg-prep ops between GetField and Call. The
        //     `start` (GetTabUp) and `start+1` (GetField) emit
        //     positions are silent — their semantic result (the
        //     resolved `math.<fn>` callable in R[A]) is known
        //     statically and never consumed by anything except
        //     the Call we're collapsing.
        if folded_ops[i] {
            // Resolve which fold this index belongs to: the start
            // (Libm1 emit site or Min2/Max2 silent GetTabUp), the
            // GetField mid-op (Min2/Max2 silent), or the Call
            // (Min2/Max2 emit site).
            let fold = math_folds.iter().find(|f| {
                f.start_idx == i
                    || (matches!(f.kind, FoldKind::Min2 | FoldKind::Max2)
                        && (f.start_idx + 1 == i || f.call_idx == i))
            });
            if let Some(fold) = fold {
                // The fold stands for the library function; leave the
                // trace at the GetTabUp, before anything of the call
                // has run, when `math.<fn>` holds something else (checked
                // in `precheck` instead when nothing in the trace can
                // change the field).
                if fold.start_idx == i && precheck.is_none() {
                    let math_key = head_proto.consts[record.ops[i].inst.c() as usize];
                    let name_key = head_proto.consts[record.ops[i + 1].inst.c() as usize];
                    let (
                        luna_core::runtime::Value::Str(math_key),
                        luna_core::runtime::Value::Str(name_key),
                    ) = (math_key, name_key)
                    else {
                        unreachable!("the fold matcher took both keys as strings");
                    };
                    let m = emit_str_key_arg(
                        module,
                        &mut bcx,
                        math_key,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    let k = emit_str_key_arg(
                        module,
                        &mut bcx,
                        name_key,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    let check_ref = module.declare_func_in_func(math_fn_check_id, bcx.func);
                    let call = bcx.ins().call(check_ref, &[m, k]);
                    let is_library = bcx.inst_results(call)[0];
                    guard!(is_library, i, rop.pc);
                }
                match fold.kind {
                    FoldKind::Libm1 if fold.start_idx == i => {
                        // Declare libm fn fresh per fold (cranelift
                        // dedups by name in the same module).
                        let mut libm_sig = module.make_signature();
                        libm_sig.params.push(AbiParam::new(types::F64));
                        libm_sig.returns.push(AbiParam::new(types::F64));
                        let libm_id = module
                            .declare_function(fold.fn_name, Linkage::Import, &libm_sig)
                            .ok()?;
                        let libm_ref = module.declare_func_in_func(libm_id, bcx.func);
                        // Libm1 always has a Reg arg_src — coerce
                        // to f64 via the existing Int→f64 / bitcast
                        // ladder based on current_kinds.
                        let arg_src = fold.arg_src.expect("Libm1 has arg_src");
                        let FoldArgSrc::Reg { reg: arg_reg } = arg_src;
                        let arg_kind = k_op(&current_kinds, off as u32 + arg_reg);
                        // The argument must be a number the trace knows as
                        // one: a numeric string is valid Lua here, and its
                        // payload is a pointer.
                        if !matches!(arg_kind, RegKind::Int | RegKind::Float) {
                            return None;
                        }
                        if is_rounding(fold.fn_name) {
                            // 5.4+: an integer is its own floor/ceil; a
                            // float's becomes an integer when it fits.
                            // When it does not (NaN, the infinities,
                            // beyond ±2^63) the result is a float, and
                            // the trace leaves at the GetTabUp — nothing
                            // of the call has run — for the interpreter.
                            let raw = bcx.use_var(regs[arg_reg as usize]);
                            let r = if matches!(arg_kind, RegKind::Float) {
                                let x = use_var_f64(&mut bcx, regs, arg_reg);
                                let r = if fold.fn_name == "floor" {
                                    bcx.ins().floor(x)
                                } else {
                                    bcx.ins().ceil(x)
                                };
                                let fits = emit_f64_fits_i64(&mut bcx, r);
                                guard!(fits, i, rop.pc);
                                bcx.ins().fcvt_to_sint(types::I64, r)
                            } else {
                                raw
                            };
                            bcx.def_var(regs[fold.dst_reg as usize], r);
                            current_kinds[off + fold.dst_reg as usize] = RegKind::Int;
                            continue;
                        }
                        let arg_f64 = if matches!(arg_kind, RegKind::Float) {
                            use_var_f64(&mut bcx, regs, arg_reg)
                        } else {
                            let raw = bcx.use_var(regs[arg_reg as usize]);
                            bcx.ins().fcvt_from_sint(types::F64, raw)
                        };
                        let call = if fold.fn_name == "atan" {
                            // Only on 5.4+ (see the matcher): atan2(y, 1).
                            let mut atan2_sig = module.make_signature();
                            atan2_sig.params.push(AbiParam::new(types::F64));
                            atan2_sig.params.push(AbiParam::new(types::F64));
                            atan2_sig.returns.push(AbiParam::new(types::F64));
                            let atan2_id = module
                                .declare_function("atan2", Linkage::Import, &atan2_sig)
                                .ok()?;
                            let atan2_ref = module.declare_func_in_func(atan2_id, bcx.func);
                            let one = bcx.ins().f64const(1.0);
                            bcx.ins().call(atan2_ref, &[arg_f64, one])
                        } else {
                            bcx.ins().call(libm_ref, &[arg_f64])
                        };
                        let r = bcx.inst_results(call)[0];
                        def_var_f64(&mut bcx, regs[fold.dst_reg as usize], r);
                        current_kinds[off + fold.dst_reg as usize] = RegKind::Float;
                    }
                    FoldKind::Libm1 => {
                        // Libm1 silent trailer (Move / Call) — folded
                        // away, no IR.
                    }
                    FoldKind::Min2 | FoldKind::Max2 if fold.call_idx == i => {
                        // 2-arg min/max. From 5.3 PUC's `math.min(a, b)`
                        // returns one of its operands as it is, so the
                        // lowering follows the recorded operand kinds:
                        //
                        //   5.1 / 5.2    → `fcmp` + `select`, as floats
                        //   Int  / Int   → cranelift `smin` / `smax`
                        //   Float/ Float → `fcmp` + `select`
                        //   otherwise    → not compiled
                        let k1 = k_op(&current_kinds, off as u32 + fold.arg1_reg);
                        let k2 = k_op(&current_kinds, off as u32 + fold.arg2_reg);
                        // `math.max` returns whichever argument wins,
                        // unconverted (5.3+), so an Int/Float pair has no
                        // static result kind: such a trace is not
                        // compiled.
                        // Anything but two numbers of one kind (strings
                        // compare too, from 5.3) is not compiled either.
                        let result_kind = match (k1, k2) {
                            // 5.1 / 5.2 convert every argument to a
                            // float (`luaL_checknumber`) and return
                            // that float, whatever the argument kinds
                            (RegKind::Int | RegKind::Float, RegKind::Int | RegKind::Float)
                                if float_only =>
                            {
                                RegKind::Float
                            }
                            (RegKind::Float, RegKind::Float) => RegKind::Float,
                            (RegKind::Int, RegKind::Int) => RegKind::Int,
                            (RegKind::Int, RegKind::Float) | (RegKind::Float, RegKind::Int) => {
                                // The winner keeps its kind, which is
                                // known only at run time. The trace
                                // continues when the first argument wins
                                // (PUC keeps it unless the second is
                                // strictly better, compared exactly) and
                                // otherwise leaves for the interpreter
                                // at the fold's GetTabUp: the folded
                                // GetTabUp / GetField never filled R[A],
                                // and the argument set-up in between
                                // only writes the call's argument slots,
                                // so running it again is harmless.
                                let a1 = bcx.use_var(regs[fold.arg1_reg as usize]);
                                let a2 = bcx.use_var(regs[fold.arg2_reg as usize]);
                                let f1 = bcx.ins().bitcast(types::F64, MemFlagsData::new(), a1);
                                let f2 = bcx.ins().bitcast(types::F64, MemFlagsData::new(), a2);
                                // max: second wins iff a1 < a2; min: iff a2 < a1.
                                let second_wins = match (fold.kind, k1) {
                                    (FoldKind::Max2, RegKind::Int) => {
                                        emit_lt_int_float(&mut bcx, a1, f2)
                                    }
                                    (FoldKind::Max2, _) => emit_lt_float_int(&mut bcx, f1, a2),
                                    (FoldKind::Min2, RegKind::Int) => {
                                        emit_lt_float_int(&mut bcx, f2, a1)
                                    }
                                    (FoldKind::Min2, _) => emit_lt_int_float(&mut bcx, a2, f1),
                                    (FoldKind::Libm1, _) => unreachable!(),
                                };
                                let first_wins = bcx.ins().bxor_imm_u(second_wins, 1);
                                guard!(first_wins, i, record.ops[fold.start_idx].pc);
                                bcx.def_var(regs[fold.dst_reg as usize], a1);
                                current_kinds[off + fold.dst_reg as usize] = k1;
                                continue;
                            }
                            _ => return None,
                        };
                        if matches!(result_kind, RegKind::Float) {
                            let a1 = use_var_as_f64(&mut bcx, regs, fold.arg1_reg, k1);
                            let a2 = use_var_as_f64(&mut bcx, regs, fold.arg2_reg, k2);
                            // PUC keeps the first argument unless the
                            // second compares strictly better — not
                            // IEEE fmin/fmax, which differ on NaN and
                            // on -0.0 vs 0.0.
                            let second_wins = match fold.kind {
                                FoldKind::Min2 => bcx.ins().fcmp(FloatCC::LessThan, a2, a1),
                                FoldKind::Max2 => bcx.ins().fcmp(FloatCC::LessThan, a1, a2),
                                FoldKind::Libm1 => unreachable!(),
                            };
                            let r = bcx.ins().select(second_wins, a2, a1);
                            def_var_f64(&mut bcx, regs[fold.dst_reg as usize], r);
                            current_kinds[off + fold.dst_reg as usize] = RegKind::Float;
                        } else {
                            // Int / Int — both operands are i64
                            // payloads holding Int values. Use
                            // signed integer min/max so the result
                            // stays Int-tagged.
                            let a1 = bcx.use_var(regs[fold.arg1_reg as usize]);
                            let a2 = bcx.use_var(regs[fold.arg2_reg as usize]);
                            let r = match fold.kind {
                                FoldKind::Min2 => bcx.ins().smin(a1, a2),
                                FoldKind::Max2 => bcx.ins().smax(a1, a2),
                                FoldKind::Libm1 => unreachable!(),
                            };
                            bcx.def_var(regs[fold.dst_reg as usize], r);
                            current_kinds[off + fold.dst_reg as usize] = RegKind::Int;
                        }
                    }
                    FoldKind::Min2 | FoldKind::Max2 => {
                        // Silent: this index is either `start_idx`
                        // (GetTabUp) or `start_idx + 1` (GetField).
                        // The Call's emit will fire at `call_idx`
                        // and produce the fold's IR.
                    }
                }
            }
            continue;
        }
        let ins = rop.inst;
        let op = ins.op();
        match op {
            Op::Jmp => {
                // Trailing back-edge (validated in the pre-emit
                // pass). The tail's `return iconst(head_pc)`
                // carries the control transfer.
            }
            Op::Move => {
                let src = bcx.use_var(regs[ins.b() as usize]);
                bcx.def_var(regs[ins.a() as usize], src);
                current_kinds[off + ins.a() as usize] = k_op(&current_kinds, off as u32 + ins.b());
            }
            Op::LoadI => {
                let imm = ins.sbx() as i64;
                let v = bcx.ins().iconst(types::I64, imm);
                bcx.def_var(regs[ins.a() as usize], v);
                current_kinds[off + ins.a() as usize] = RegKind::Int;
                known_int[off + ins.a() as usize] = Some(imm);
            }
            Op::LoadF => {
                // R[A] := sBx as f64. Bitcast result to i64
                // bit-pattern so the reg's storage stays uniform.
                let f = ins.sbx() as f64;
                let v = bcx.ins().f64const(f);
                def_var_f64(&mut bcx, regs[ins.a() as usize], v);
                current_kinds[off + ins.a() as usize] = RegKind::Float;
            }
            Op::LoadNil => {
                // R[A..=A+B] := nil. NIL raw payload bits
                // are 0; emit one iconst(0) and def_var it into each
                // target slot, marking current_kinds = Nil so the
                // exit-tag derivation (kinds_to_exit_tags)
                // produces ExitTag::Nil for slots the trace touched.
                let a_us = ins.a() as usize;
                let b_us = ins.b() as usize;
                let zero = bcx.ins().iconst(types::I64, 0);
                for k in 0..=b_us {
                    bcx.def_var(regs[a_us + k], zero);
                    current_kinds[off + a_us + k] = RegKind::Nil;
                }
            }
            Op::LoadK => {
                let bx = ins.bx() as usize;
                let (v, k) = match head_proto.consts[bx] {
                    luna_core::runtime::Value::Int(n) => {
                        known_int[off + ins.a() as usize] = Some(n);
                        (bcx.ins().iconst(types::I64, n), RegKind::Int)
                    }
                    luna_core::runtime::Value::Float(f) => {
                        let fv = bcx.ins().f64const(f);
                        let bits = bcx.ins().bitcast(types::I64, MemFlagsData::new(), fv);
                        (bits, RegKind::Float)
                    }
                    _ => unreachable!("pre-emit gates Int / Float consts"),
                };
                bcx.def_var(regs[ins.a() as usize], v);
                current_kinds[off + ins.a() as usize] = k;
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => {
                let kb = kind!(ins.b());
                let kc = kind!(ins.c());
                // A string operand is coerced (or has `__add` & co. in its
                // metatable); only numbers are lowered, since the payload of
                // anything else is a pointer.
                let number = |k| matches!(k, RegKind::Int | RegKind::Float);
                if !number(kb) || !number(kc) {
                    return None;
                }
                // Op::Pow always returns Float in Lua 5.4+ (matches
                // `pow(f64, f64) -> f64`); coerce Int operands to
                // Float via fcvt_from_sint.
                if matches!(op, Op::Pow) {
                    let lhs = match kb {
                        RegKind::Float => use_var_f64(&mut bcx, regs, ins.b()),
                        _ => {
                            let raw = bcx.use_var(regs[ins.b() as usize]);
                            bcx.ins().fcvt_from_sint(types::F64, raw)
                        }
                    };
                    let rhs = match kc {
                        RegKind::Float => use_var_f64(&mut bcx, regs, ins.c()),
                        _ => {
                            let raw = bcx.use_var(regs[ins.c() as usize]);
                            bcx.ins().fcvt_from_sint(types::F64, raw)
                        }
                    };
                    let mut pow_sig = module.make_signature();
                    pow_sig.params.push(AbiParam::new(types::F64));
                    pow_sig.params.push(AbiParam::new(types::F64));
                    pow_sig.returns.push(AbiParam::new(types::F64));
                    let pow_id = module
                        .declare_function("pow", Linkage::Import, &pow_sig)
                        .ok()?;
                    let pow_ref = module.declare_func_in_func(pow_id, bcx.func);
                    let call = bcx.ins().call(pow_ref, &[lhs, rhs]);
                    let mut r = bcx.inst_results(call)[0];
                    // 5.4+ `luai_numpow` squares by multiplying, which can
                    // differ from `pow` in the last bit
                    if !opts.pre53 {
                        let two = bcx.ins().f64const(2.0);
                        let is_two = bcx.ins().fcmp(FloatCC::Equal, rhs, two);
                        let sq = bcx.ins().fmul(lhs, lhs);
                        r = bcx.ins().select(is_two, sq, r);
                    }
                    def_var_f64(&mut bcx, regs[ins.a() as usize], r);
                    current_kinds[off + ins.a() as usize] = RegKind::Float;
                    continue;
                }
                // Float path when either operand is known-Float.
                // Both must be Float — mixed Int+Float would
                // semantically coerce to Float in Lua, but the
                // trace's kind tracker bails to avoid the
                // ambiguous emit.
                let float_path = matches!(kb, RegKind::Float) || matches!(kc, RegKind::Float);
                if float_path {
                    if !matches!(kb, RegKind::Float) || !matches!(kc, RegKind::Float) {
                        return None;
                    }
                    let lhs = use_var_f64(&mut bcx, regs, ins.b());
                    let rhs = use_var_f64(&mut bcx, regs, ins.c());
                    let r = match op {
                        Op::Add => bcx.ins().fadd(lhs, rhs),
                        Op::Sub => bcx.ins().fsub(lhs, rhs),
                        Op::Mul => bcx.ins().fmul(lhs, rhs),
                        Op::Div => bcx.ins().fdiv(lhs, rhs),
                        _ => unreachable!(),
                    };
                    def_var_f64(&mut bcx, regs[ins.a() as usize], r);
                    current_kinds[off + ins.a() as usize] = RegKind::Float;
                } else {
                    // Op::Div on Int operands would still coerce
                    // to Float in Lua 5.4+. Bail to be safe.
                    if matches!(op, Op::Div) {
                        return None;
                    }
                    let lhs = bcx.use_var(regs[ins.b() as usize]);
                    let rhs = bcx.use_var(regs[ins.c() as usize]);
                    let r = match op {
                        Op::Add => bcx.ins().iadd(lhs, rhs),
                        Op::Sub => bcx.ins().isub(lhs, rhs),
                        Op::Mul => bcx.ins().imul(lhs, rhs),
                        _ => unreachable!(),
                    };
                    bcx.def_var(regs[ins.a() as usize], r);
                    current_kinds[off + ins.a() as usize] = RegKind::Int;
                }
            }
            // 3-reg Int ops. The cases the machine instruction gets
            // wrong for Lua — a zero divisor (Lua raises), a shift count
            // outside 0..=63 (Lua shifts the other way or gives 0) — leave
            // the trace at the op so the interpreter does them; a -1
            // divisor (the machine traps on minint) is done inline.
            Op::IDiv | Op::Mod | Op::BAnd | Op::BOr | Op::BXor | Op::Shl | Op::Shr => {
                // Lowered for two integers only: a float operand makes
                // `//` and `%` float ops and the bitwise ops convert or
                // raise; a string is coerced.
                let kb = kind!(ins.b());
                let kc = kind!(ins.c());
                if !matches!(kb, RegKind::Int) || !matches!(kc, RegKind::Int) {
                    return None;
                }
                let lhs = bcx.use_var(regs[ins.b() as usize]);
                let rhs = bcx.use_var(regs[ins.c() as usize]);
                let r = match (op, rc_const) {
                    // a constant divisor needs neither guard (and the
                    // machine division by a constant is strength-reduced)
                    (Op::IDiv | Op::Mod, Some(k)) if k != 0 && k != -1 => {
                        emit_floor_divmod_by(&mut bcx, op, lhs, k)
                    }
                    // a constant shift count is a single machine shift
                    (Op::Shl | Op::Shr, Some(k)) => {
                        let n = if op == Op::Shr { k.wrapping_neg() } else { k };
                        if n <= -64 || n >= 64 {
                            bcx.ins().iconst(types::I64, 0)
                        } else if n >= 0 {
                            bcx.ins().ishl_imm_u(lhs, n)
                        } else {
                            bcx.ins().ushr_imm_u(lhs, -n)
                        }
                    }
                    _ => match op {
                        Op::IDiv | Op::Mod => {
                            // A zero divisor is the interpreter's error to
                            // raise: leave the trace at this op.
                            let zero = bcx.ins().iconst(types::I64, 0);
                            let is_zero = bcx.ins().icmp(IntCC::Equal, rhs, zero);
                            let cont_blk = bcx.create_block();
                            let exit_blk = bcx.create_block();
                            bcx.ins().brif(is_zero, exit_blk, &[], cont_blk, &[]);
                            bcx.switch_to_block(exit_blk);
                            bcx.seal_block(exit_blk);
                            guard_exit!(rop.pc, i);
                            bcx.switch_to_block(cont_blk);
                            bcx.seal_block(cont_blk);
                            emit_floor_divmod(&mut bcx, op, lhs, rhs)
                        }
                        Op::BAnd => bcx.ins().band(lhs, rhs),
                        Op::BOr => bcx.ins().bor(lhs, rhs),
                        Op::BXor => bcx.ins().bxor(lhs, rhs),
                        Op::Shl | Op::Shr => {
                            let wide = bcx.ins().icmp_imm_u(IntCC::UnsignedGreaterThan, rhs, 63);
                            let cont_blk = bcx.create_block();
                            let exit_blk = bcx.create_block();
                            bcx.ins().brif(wide, exit_blk, &[], cont_blk, &[]);
                            bcx.switch_to_block(exit_blk);
                            bcx.seal_block(exit_blk);
                            guard_exit!(rop.pc, i);
                            bcx.switch_to_block(cont_blk);
                            bcx.seal_block(cont_blk);
                            if op == Op::Shl {
                                bcx.ins().ishl(lhs, rhs)
                            } else {
                                bcx.ins().ushr(lhs, rhs)
                            }
                        }
                        _ => unreachable!("whitelist gated above"),
                    },
                };
                bcx.def_var(regs[ins.a() as usize], r);
                current_kinds[off + ins.a() as usize] = RegKind::Int;
            }
            Op::Unm | Op::BNot => {
                let kb = k_op(&current_kinds, off as u32 + ins.b());
                if !matches!(kb, RegKind::Int)
                    && !(matches!(op, Op::Unm) && matches!(kb, RegKind::Float))
                {
                    return None;
                }
                if matches!(op, Op::Unm) && matches!(kb, RegKind::Float) {
                    // Float negation.
                    let src = use_var_f64(&mut bcx, regs, ins.b());
                    let r = bcx.ins().fneg(src);
                    def_var_f64(&mut bcx, regs[ins.a() as usize], r);
                    current_kinds[off + ins.a() as usize] = RegKind::Float;
                } else {
                    let src = bcx.use_var(regs[ins.b() as usize]);
                    let r = match op {
                        Op::Unm => bcx.ins().ineg(src),
                        Op::BNot => bcx.ins().bnot(src),
                        _ => unreachable!("whitelist gated above"),
                    };
                    bcx.def_var(regs[ins.a() as usize], r);
                    current_kinds[off + ins.a() as usize] = RegKind::Int;
                }
            }
            Op::EqK => {
                // `R[A] == const[B]` — Int and Float consts both
                // valid (pre-emit gated above). Emit icmp eq for
                // Int + Int, fcmp eq for Float + Float; a number
                // against the other number kind bails, against a
                // non-number is never equal.
                let bx = ins.b() as usize;
                let ka = k_op(&current_kinds, off as u32 + ins.a());
                let cond = match head_proto.consts[bx] {
                    luna_core::runtime::Value::Int(n) => {
                        if matches!(ka, RegKind::Float) {
                            return None;
                        }
                        match eq_lowering(ka, RegKind::Int) {
                            EqLowering::Unequal => bcx.ins().iconst(types::I8, i64::from(!ins.k())),
                            lowering => {
                                if lowering == EqLowering::Unknown {
                                    dispatchable = false;
                                    dispatch_off_reason =
                                        dispatch_off_reason.or(Some("cmp:unknown-kind"));
                                }
                                let lhs = bcx.use_var(regs[ins.a() as usize]);
                                let rhs = bcx.ins().iconst(types::I64, n);
                                let int_cc = if ins.k() {
                                    IntCC::Equal
                                } else {
                                    IntCC::NotEqual
                                };
                                bcx.ins().icmp(int_cc, lhs, rhs)
                            }
                        }
                    }
                    luna_core::runtime::Value::Float(f) => {
                        if !matches!(ka, RegKind::Float) {
                            return None;
                        }
                        let lhs = use_var_f64(&mut bcx, regs, ins.a());
                        let rhs = bcx.ins().f64const(f);
                        let float_cc = if ins.k() {
                            FloatCC::Equal
                        } else {
                            FloatCC::NotEqual
                        };
                        bcx.ins().fcmp(float_cc, lhs, rhs)
                    }
                    _ => unreachable!("pre-emit gates Int / Float const only"),
                };

                let continue_blk = bcx.create_block();
                let side_exit_blk = bcx.create_block();
                bcx.ins().brif(cond, continue_blk, &[], side_exit_blk, &[]);

                bcx.switch_to_block(side_exit_blk);
                bcx.seal_block(side_exit_blk);
                let side_exit_pc = rop.pc + 2;
                // at depth>0, the side-exit must
                // materialise the inlined frames before the interp can
                // resume at the cmp's PC. See the matching Lt/Le/Eq
                // arm below for the chain-build details.
                if !call_chain.is_empty() {
                    // Capture head's resume pc BEFORE the innermost
                    // override — `call_chain[0].pc` is the outermost
                    // self-rec Call's `pc + 1` (= trace head's
                    // post-Call resume).
                    let head_resume_pc = call_chain[0].pc;
                    let mut snapshot: Vec<FrameMaterializeInfo> = call_chain.clone();
                    if let Some(last) = snapshot.last_mut() {
                        last.pc = side_exit_pc;
                    }
                    let chain_rc: TArc<[FrameMaterializeInfo]> = snapshot.into();
                    let chain_ptr = TArc::as_ptr(&chain_rc) as *const FrameMaterializeInfo as i64;
                    let chain_len = chain_rc.len() as i64;
                    let site_idx = per_exit_inline_vec.len() as u32;
                    // materialise live Sinkable sites
                    // BEFORE the frame_materialize_frames helper
                    // pushes the inline frames. The window-sized
                    // snapshot updates in-place so per_exit_inline's
                    // kinds entry reflects materialised slots.
                    let mut kinds_snapshot: Vec<RegKind> = current_kinds.clone();
                    let mat_count = emit_materialize_live_sunk(
                        &mut bcx,
                        &mut module,
                        mat_sunk_id,
                        &escape,
                        &virt_vars,
                        &virt_kinds,
                        &regs_full,
                        &op_offsets,
                        i,
                        &mut kinds_snapshot,
                        head_proto,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    materialize_emit_count += mat_count;
                    let inline_side_box_0: Box<TCellPtr> = Box::new(TCellPtr::null());
                    let _inline_side_cell_addr_0 = (&*inline_side_box_0) as *const TCellPtr as i64;
                    let chain_for_helper = chain_rc.clone();
                    per_exit_inline_vec.push((
                        side_exit_pc,
                        head_resume_pc,
                        kinds_snapshot,
                        chain_rc,
                        inline_side_box_0,
                    ));
                    let n_arg = bcx.ins().iconst(types::I64, chain_len);
                    let ptr_arg = emit_chain_ptr_arg(
                        &mut module,
                        &mut bcx,
                        &chain_for_helper,
                        chain_ptr,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    let mat_ref = module.declare_func_in_func(materialize_id, bcx.func);
                    let _ = bcx.ins().call(mat_ref, &[n_arg, ptr_arg]);
                    emit_store_back_and_return_site(
                        &mut bcx,
                        &regs_full[..window_size_us],
                        &stored,
                        reg_state,
                        site_idx,
                        side_exit_pc,
                        flush_ctx.as_ref(),
                        0i64,
                        trace_fn_sig_ref,
                    );
                } else {
                    // materialise every live
                    // Sinkable site at this depth=0 cmp side-exit.
                    // The snapshot carries `RegKind::Table` for each
                    // materialised caller-window slot so the
                    // dispatcher unpacks the heap pointer correctly
                    // on deopt.
                    let mut snapshot: Vec<RegKind> = current_kinds[..max_stack].to_vec();
                    let mat_count = emit_materialize_live_sunk(
                        &mut bcx,
                        &mut module,
                        mat_sunk_id,
                        &escape,
                        &virt_vars,
                        &virt_kinds,
                        &regs_full,
                        &op_offsets,
                        i,
                        &mut snapshot,
                        head_proto,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    materialize_emit_count += mat_count;
                    let tag_side_box_0: Box<TCellPtr> = Box::new(TCellPtr::null());
                    let _tag_side_cell_addr_0 = (&*tag_side_box_0) as *const TCellPtr as i64;
                    let tag_side_local_0 = per_exit_kinds.len() as u32;
                    per_exit_kinds.push((side_exit_pc, snapshot, tag_side_box_0));
                    // store_back only writes caller window — depth>0 scratch
                    // slots stay out of the dispatcher's reg_state restore.
                    emit_tagged_exit(
                        &mut bcx,
                        &mut module,
                        suppress_admit_id,
                        &regs_full[..max_stack],
                        &stored,
                        reg_state,
                        side_exit_pc,
                        record.head_pc,
                        tag_side_local_0,
                        flush_ctx.as_ref(),
                        trace_fn_sig_ref,
                    );
                }

                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
            }
            Op::Test => {
                // `if (not R[A] == K) then pc++`.
                //
                // Known kind → compile-time fold (`truthy_known`
                // table). Match recorded → no IR; mismatch → bail.
                // Unset → emit runtime guard via
                // `luna_jit_stack_tag(A)` + `(tag > 1) == truthy`
                // check; runtime mismatch → deopt store_back +
                // return test.pc. The Subsequent Jmp (if TookJmp)
                // is consumed_by_cmp by the pre-emit pass.
                let a_kind = k_op(&current_kinds, off as u32 + ins.a());
                let truthy_known: Option<bool> = match a_kind {
                    RegKind::Int
                    | RegKind::Float
                    | RegKind::Table
                    | RegKind::Closure
                    | RegKind::Str => Some(true),
                    RegKind::Nil => Some(false),
                    RegKind::Unset | RegKind::Unknown => None,
                    // the trace reads it, so it is never held on the stack
                    RegKind::StackHeld => return None,
                };
                let k_bit = ins.k();
                let recorded_passed = matches!(cmp_dirs[i], Some(CmpDir::SkippedJmp));
                if let Some(truthy) = truthy_known {
                    let test_passed = truthy != k_bit;
                    if test_passed != recorded_passed {
                        // Provably can't reproduce recorded
                        // direction; bail compile.
                        return None;
                    }
                    // Test consumed; no IR. Match guaranteed at
                    // compile time.
                } else {
                    // Runtime tag-based truthy guard.
                    let slot_arg = bcx.ins().iconst(types::I64, ins.a() as i64);
                    let stack_tag_ref = module.declare_func_in_func(stack_tag_id, bcx.func);
                    let tag_call = bcx.ins().call(stack_tag_ref, &[slot_arg]);
                    let tag = bcx.inst_results(tag_call)[0];
                    let one = bcx.ins().iconst(types::I64, 1);
                    let is_truthy = bcx.ins().icmp(IntCC::UnsignedGreaterThan, tag, one);
                    // Op::Test: test_passed_runtime = !is_truthy == k_bit
                    let not_truthy = bcx.ins().bxor_imm_u(is_truthy, 1);
                    let k_bit_const = bcx.ins().iconst(types::I8, k_bit as i64);
                    let test_passed_runtime = bcx.ins().icmp(IntCC::Equal, not_truthy, k_bit_const);
                    let recorded_const = bcx.ins().iconst(types::I8, recorded_passed as i64);
                    let ok = bcx
                        .ins()
                        .icmp(IntCC::Equal, test_passed_runtime, recorded_const);
                    let cont = bcx.create_block();
                    let deopt = bcx.create_block();
                    bcx.ins().brif(ok, cont, &[], deopt, &[]);
                    bcx.switch_to_block(deopt);
                    bcx.seal_block(deopt);
                    // restored with the kinds the registers have here
                    guard_exit!(rop.pc, i);
                    bcx.switch_to_block(cont);
                    bcx.seal_block(cont);
                }
            }
            Op::TestSet => {
                // `if truthy(R[B]) == K then
                // R[A] = R[B] else pc++`.
                //
                // Known kind → compile-time fold + emit Move
                // on `TookJmp` recorded path.
                // Unset → emit runtime guard via stack_tag;
                // when match + TookJmp recorded, emit Move under
                // the `cont` block (so deopt path skips the Move).
                let b_kind = k_op(&current_kinds, off as u32 + ins.b());
                let truthy_known: Option<bool> = match b_kind {
                    RegKind::Int
                    | RegKind::Float
                    | RegKind::Table
                    | RegKind::Closure
                    | RegKind::Str => Some(true),
                    RegKind::Nil => Some(false),
                    RegKind::Unset | RegKind::Unknown => None,
                    // the trace reads it, so it is never held on the stack
                    RegKind::StackHeld => return None,
                };
                let k_bit = ins.k();
                let recorded_passed = matches!(cmp_dirs[i], Some(CmpDir::TookJmp));
                if let Some(truthy) = truthy_known {
                    let test_passed = truthy == k_bit;
                    if test_passed != recorded_passed {
                        return None;
                    }
                    if test_passed {
                        let v = bcx.use_var(regs[ins.b() as usize]);
                        bcx.def_var(regs[ins.a() as usize], v);
                        current_kinds[off + ins.a() as usize] = b_kind;
                    }
                } else {
                    // Runtime guard. Same shape as Op::Test
                    // but the basis is `is_truthy` (not `!is_truthy`).
                    let slot_arg = bcx.ins().iconst(types::I64, ins.b() as i64);
                    let stack_tag_ref = module.declare_func_in_func(stack_tag_id, bcx.func);
                    let tag_call = bcx.ins().call(stack_tag_ref, &[slot_arg]);
                    let tag = bcx.inst_results(tag_call)[0];
                    let one = bcx.ins().iconst(types::I64, 1);
                    let is_truthy = bcx.ins().icmp(IntCC::UnsignedGreaterThan, tag, one);
                    let k_bit_const = bcx.ins().iconst(types::I8, k_bit as i64);
                    let test_passed_runtime = bcx.ins().icmp(IntCC::Equal, is_truthy, k_bit_const);
                    let recorded_const = bcx.ins().iconst(types::I8, recorded_passed as i64);
                    let ok = bcx
                        .ins()
                        .icmp(IntCC::Equal, test_passed_runtime, recorded_const);
                    let cont = bcx.create_block();
                    let deopt = bcx.create_block();
                    bcx.ins().brif(ok, cont, &[], deopt, &[]);
                    bcx.switch_to_block(deopt);
                    bcx.seal_block(deopt);
                    // restored with the kinds the registers have here
                    guard_exit!(rop.pc, i);
                    bcx.switch_to_block(cont);
                    bcx.seal_block(cont);
                    if recorded_passed {
                        let v = bcx.use_var(regs[ins.b() as usize]);
                        bcx.def_var(regs[ins.a() as usize], v);
                        current_kinds[off + ins.a() as usize] =
                            k_op(&current_kinds, off as u32 + ins.b());
                    }
                }
            }
            Op::Lt | Op::Le | Op::Eq => {
                // Lua semantics: `if (R[A] op R[B]) ~= K then pc++`.
                // Operand kinds must match — Int → icmp; Float →
                // fcmp; mixed → bail.
                //
                // `cmp_dirs[i]` decides which direction the cmp
                // recorded: TookJmp (cond folds K so true ⇒ took
                // Jmp → continue) or SkippedJmp (cond folds !K so
                // true ⇒ skipped Jmp → continue; side-exit goes
                // to the Jmp's target). The pre-emit pass already
                // validated `i+1 < effective_end`.
                let dir = cmp_dirs[i].expect("cmp dir set in pre-emit");
                let invert = matches!(dir, CmpDir::SkippedJmp);
                let k_effective = if invert { !ins.k() } else { ins.k() };
                let ka = kind!(ins.a());
                let kb = kind!(ins.b());
                let float_path = matches!(ka, RegKind::Float) || matches!(kb, RegKind::Float);
                let cond = if float_path {
                    if !matches!(ka, RegKind::Float) || !matches!(kb, RegKind::Float) {
                        return None;
                    }
                    let lhs = use_var_f64(&mut bcx, regs, ins.a());
                    let rhs = use_var_f64(&mut bcx, regs, ins.b());
                    let float_cc = match op {
                        Op::Lt => FloatCC::LessThan,
                        Op::Le => FloatCC::LessThanOrEqual,
                        Op::Eq => FloatCC::Equal,
                        _ => unreachable!("whitelist gated above"),
                    };
                    let c = bcx.ins().fcmp(float_cc, lhs, rhs);
                    // negate the ordered compare rather than flip the
                    // condition: `not (a < b)` holds for NaN, `a >= b`
                    // does not, and the aarch64 backend lowers no
                    // unordered-or conditions
                    if k_effective {
                        c
                    } else {
                        bcx.ins().icmp_imm_u(IntCC::Equal, c, 0)
                    }
                } else if op == Op::Eq {
                    let lhs = bcx.use_var(regs[ins.a() as usize]);
                    let rhs = bcx.use_var(regs[ins.b() as usize]);
                    let int_cc = if k_effective {
                        IntCC::Equal
                    } else {
                        IntCC::NotEqual
                    };
                    match eq_lowering(ka, kb) {
                        EqLowering::Payload => bcx.ins().icmp(int_cc, lhs, rhs),
                        EqLowering::Unequal => bcx.ins().iconst(types::I8, i64::from(!k_effective)),
                        EqLowering::Identity(kind) => {
                            // two distinct objects can still be equal (`__eq`,
                            // equal long strings); the interpreter decides those
                            let same = bcx.ins().icmp(IntCC::Equal, lhs, rhs);
                            let decided = if kind == RegKind::Table {
                                let no_mt = |bcx: &mut FunctionBuilder<'_>, t| {
                                    let mt = bcx.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        t,
                                        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
                                    );
                                    bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0)
                                };
                                let l = no_mt(&mut bcx, lhs);
                                let r = no_mt(&mut bcx, rhs);
                                bcx.ins().band(l, r)
                            } else {
                                let short = |bcx: &mut FunctionBuilder<'_>, s| {
                                    let b = bcx.ins().load(
                                        types::I8,
                                        MemFlagsData::trusted(),
                                        s,
                                        crate::jit_backend::STR_SHORT_OFFSET as i32,
                                    );
                                    bcx.ins().icmp_imm_u(IntCC::NotEqual, b, 0)
                                };
                                let l = short(&mut bcx, lhs);
                                let r = short(&mut bcx, rhs);
                                bcx.ins().band(l, r)
                            };
                            let ok = bcx.ins().bor(same, decided);
                            guard!(ok, i, rop.pc);
                            bcx.ins().icmp(int_cc, lhs, rhs)
                        }
                        EqLowering::Unknown => {
                            dispatchable = false;
                            dispatch_off_reason = dispatch_off_reason.or(Some("cmp:unknown-kind"));
                            bcx.ins().icmp(int_cc, lhs, rhs)
                        }
                    }
                } else {
                    // only integers order by their payload
                    match (ka, kb) {
                        (RegKind::Int, RegKind::Int) => {}
                        (
                            RegKind::Unset | RegKind::Unknown,
                            RegKind::Int | RegKind::Unset | RegKind::Unknown,
                        )
                        | (RegKind::Int, RegKind::Unset | RegKind::Unknown) => {
                            dispatchable = false;
                            dispatch_off_reason = dispatch_off_reason.or(Some("cmp:unknown-kind"));
                        }
                        _ => return None,
                    }
                    let lhs = bcx.use_var(regs[ins.a() as usize]);
                    let rhs = bcx.use_var(regs[ins.b() as usize]);
                    let int_cc = match (op, k_effective) {
                        (Op::Lt, true) => IntCC::SignedLessThan,
                        (Op::Lt, false) => IntCC::SignedGreaterThanOrEqual,
                        (Op::Le, true) => IntCC::SignedLessThanOrEqual,
                        (Op::Le, false) => IntCC::SignedGreaterThan,
                        _ => unreachable!("whitelist gated above"),
                    };
                    bcx.ins().icmp(int_cc, lhs, rhs)
                };

                let continue_blk = bcx.create_block();
                let side_exit_blk = bcx.create_block();
                bcx.ins().brif(cond, continue_blk, &[], side_exit_blk, &[]);

                // Side-exit PC depends on the recorded direction:
                //   TookJmp    → interp's `pc++` lands at cmp_pc + 2.
                //   SkippedJmp → interp would have taken the Jmp;
                //                resume at the Jmp's target.
                let side_exit_pc: u32 = match dir {
                    CmpDir::TookJmp => rop.pc + 2,
                    CmpDir::SkippedJmp => {
                        let jmp_pc = (rop.pc + 1) as usize;
                        let jmp_inst = head_proto.code[jmp_pc];
                        let pc_after_jmp = (rop.pc as i64) + 2;
                        (pc_after_jmp + jmp_inst.sj() as i64) as u32
                    }
                };
                bcx.switch_to_block(side_exit_blk);
                bcx.seal_block(side_exit_blk);
                // at depth>0, snapshot the live
                // `call_chain` (each cmp@d>0 site has its OWN chain;
                // a single global depth-indexed array loops fib
                // forever because sibling Calls produce wrong
                // chains under the depth-indexed lookup). The
                // innermost frame's pc is overwritten with this
                // site's side-exit PC so the materialize helper
                // stays PC-agnostic — it just pushes whatever
                // metadata says.
                if !call_chain.is_empty() {
                    let head_resume_pc = call_chain[0].pc;
                    let mut snapshot: Vec<FrameMaterializeInfo> = call_chain.clone();
                    if let Some(last) = snapshot.last_mut() {
                        last.pc = side_exit_pc;
                    }
                    let chain_rc: TArc<[FrameMaterializeInfo]> = snapshot.into();
                    let chain_ptr = TArc::as_ptr(&chain_rc) as *const FrameMaterializeInfo as i64;
                    let chain_len = chain_rc.len() as i64;
                    let site_idx = per_exit_inline_vec.len() as u32;
                    // materialise live Sinkable sites
                    // (depth=0 + depth>0) before frame-mat helper
                    // pushes the inline frames.
                    let mut kinds_snapshot: Vec<RegKind> = current_kinds.clone();
                    let mat_count = emit_materialize_live_sunk(
                        &mut bcx,
                        &mut module,
                        mat_sunk_id,
                        &escape,
                        &virt_vars,
                        &virt_kinds,
                        &regs_full,
                        &op_offsets,
                        i,
                        &mut kinds_snapshot,
                        head_proto,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    materialize_emit_count += mat_count;
                    let inline_side_box_1: Box<TCellPtr> = Box::new(TCellPtr::null());
                    let _inline_side_cell_addr_1 = (&*inline_side_box_1) as *const TCellPtr as i64;
                    let chain_for_helper = chain_rc.clone();
                    per_exit_inline_vec.push((
                        side_exit_pc,
                        head_resume_pc,
                        kinds_snapshot,
                        chain_rc,
                        inline_side_box_1,
                    ));
                    let n_arg = bcx.ins().iconst(types::I64, chain_len);
                    let ptr_arg = emit_chain_ptr_arg(
                        &mut module,
                        &mut bcx,
                        &chain_for_helper,
                        chain_ptr,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    let mat_ref = module.declare_func_in_func(materialize_id, bcx.func);
                    let _ = bcx.ins().call(mat_ref, &[n_arg, ptr_arg]);
                    emit_store_back_and_return_site(
                        &mut bcx,
                        &regs_full[..window_size_us],
                        &stored,
                        reg_state,
                        site_idx,
                        side_exit_pc,
                        flush_ctx.as_ref(),
                        0i64,
                        trace_fn_sig_ref,
                    );
                } else {
                    // materialise-on-deopt for
                    // depth=0 cmp's live Sinkable sites.
                    let mut snapshot: Vec<RegKind> = current_kinds[..max_stack].to_vec();
                    let mat_count = emit_materialize_live_sunk(
                        &mut bcx,
                        &mut module,
                        mat_sunk_id,
                        &escape,
                        &virt_vars,
                        &virt_kinds,
                        &regs_full,
                        &op_offsets,
                        i,
                        &mut snapshot,
                        head_proto,
                        opts.aot,
                        &mut defined_aot_data,
                    );
                    materialize_emit_count += mat_count;
                    let tag_side_box_1: Box<TCellPtr> = Box::new(TCellPtr::null());
                    let _tag_side_cell_addr_1 = (&*tag_side_box_1) as *const TCellPtr as i64;
                    let tag_side_local_1 = per_exit_kinds.len() as u32;
                    per_exit_kinds.push((side_exit_pc, snapshot, tag_side_box_1));
                    emit_tagged_exit(
                        &mut bcx,
                        &mut module,
                        suppress_admit_id,
                        &regs_full[..max_stack],
                        &stored,
                        reg_state,
                        side_exit_pc,
                        record.head_pc,
                        tag_side_local_1,
                        flush_ctx.as_ref(),
                        trace_fn_sig_ref,
                    );
                }

                // Continue: subsequent ops emit here.
                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
            }
            Op::NewTable => {
                // sunk path: skip the heap alloc helper.
                // The site's virt slot Variables (allocated pre-emit)
                // hold the array elements directly. `current_kinds`
                // for the site's slot stays at its entry value
                // (Unset → maps to ExitTag::Untouched, so the
                // dispatcher carries the entry tag in the restore).
                if let Some(OpAction::NewTableSite { site_idx }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && virt_vars[site_idx as usize].is_some()
                {
                    continue;
                }
                let func_ref = module.declare_func_in_func(new_table_id, bcx.func);
                let call = bcx.ins().call(func_ref, &[]);
                let t = bcx.inst_results(call)[0];
                bcx.def_var(regs[ins.a() as usize], t);
                current_kinds[off + ins.a() as usize] = RegKind::Table;
            }
            Op::GetI => {
                // sunk path: a GetI from a Sinkable site
                // at a key in `1..=cap` becomes a `use_var` of the
                // matching virt slot Variable, with kind carried
                // from `virt_kinds`.
                if let Some(OpAction::GetIRead { site_idx, key }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && let Some(vars) = virt_vars[site_idx as usize].as_ref()
                {
                    let slot = (key as usize) - 1;
                    let v = bcx.use_var(vars[slot]);
                    bcx.def_var(regs[ins.a() as usize], v);
                    let k = virt_kinds[site_idx as usize]
                        .as_ref()
                        .expect("Sinkable site has virt_kinds")[slot];
                    current_kinds[off + ins.a() as usize] = k;
                    continue;
                }
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.b()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.b() as usize]);
                let k_imm = bcx.ins().iconst(types::I64, ins.c() as i64);
                // GetX inference: look at the immediate next op. The read
                // is checked against it, so a value of another type (or
                // a table with a metatable) leaves the trace here.
                let inferred = if i + 1 < effective_end {
                    infer_getx_exit_lookahead(ins.a(), &record.ops[i + 1..effective_end])
                } else {
                    None
                };
                if let Some((kind, want)) = getx_want(inferred) {
                    let v = checked_read!(get_int_checked_id, t, k_imm, want, rop.pc, i);
                    bcx.def_var(regs[ins.a() as usize], v);
                    current_kinds[off + ins.a() as usize] = kind;
                } else {
                    let func_ref = module.declare_func_in_func(get_int_id, bcx.func);
                    let call = bcx.ins().call(func_ref, &[t, k_imm]);
                    let v = bcx.inst_results(call)[0];
                    bcx.def_var(regs[ins.a() as usize], v);
                    // the value's type is not known: the register's
                    // earlier kind no longer describes it
                    current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                    dispatchable = false;
                    dispatch_off_reason = dispatch_off_reason.or(Some("GetI:inference-fail"));
                }
            }
            Op::GetTable => {
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.b()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.b() as usize]);
                let key = bcx.use_var(regs[ins.c() as usize]);
                let inferred = if i + 1 < effective_end {
                    infer_getx_exit_lookahead(ins.a(), &record.ops[i + 1..effective_end])
                } else {
                    None
                };
                // the helper reads the key as an integer
                let key_is_int = matches!(k_op(&current_kinds, off as u32 + ins.c()), RegKind::Int);
                match getx_want(inferred) {
                    Some((kind, want)) if key_is_int => {
                        let v = checked_read!(get_int_checked_id, t, key, want, rop.pc, i);
                        bcx.def_var(regs[ins.a() as usize], v);
                        current_kinds[off + ins.a() as usize] = kind;
                    }
                    _ => {
                        let func_ref = module.declare_func_in_func(get_int_id, bcx.func);
                        let call = bcx.ins().call(func_ref, &[t, key]);
                        let v = bcx.inst_results(call)[0];
                        bcx.def_var(regs[ins.a() as usize], v);
                        // as for GetI
                        current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                        dispatchable = false;
                        dispatch_off_reason =
                            dispatch_off_reason.or(Some("GetTable:inference-fail"));
                    }
                }
            }
            Op::SetField => {
                // sunk path: when escape sweep tagged
                // SetFieldSunkWrite, def_var the source register into
                // the matching virt slot (array_cap + hash_slot) +
                // propagate the source RegKind into virt_kinds.
                if let Some(OpAction::SetFieldSunkWrite {
                    site_idx,
                    hash_slot,
                }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && virt_vars[site_idx as usize].is_some()
                {
                    let array_cap = escape.sites[site_idx as usize].array_cap as usize;
                    let slot = array_cap + hash_slot as usize;
                    let src_kind = current_kinds[off + ins.c() as usize];
                    let v = bcx.use_var(regs[ins.c() as usize]);
                    let vars = virt_vars[site_idx as usize]
                        .as_ref()
                        .expect("Sinkable site has virt_vars");
                    bcx.def_var(vars[slot], v);
                    let kinds_vec = virt_kinds[site_idx as usize]
                        .as_mut()
                        .expect("Sinkable site has virt_kinds");
                    kinds_vec[slot] = src_kind;
                    continue;
                }
                // helper path: R[A][K[B]:string] := R[C].
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.a()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.a() as usize]);
                let key_v = match head_proto.consts[ins.b() as usize] {
                    luna_core::runtime::Value::Str(s) => s,
                    _ => unreachable!("pre-emit gates Str const at K[B]"),
                };
                let key_arg =
                    emit_str_key_arg(module, &mut bcx, key_v, opts.aot, &mut defined_aot_data);
                let val_kind = k_op(&current_kinds, off as u32 + ins.c());
                // a value of unknown kind cannot be tagged for the table
                if val_kind.untyped() {
                    return None;
                }
                let val = bcx.use_var(regs[ins.c() as usize]);
                let done = emit_table_set(
                    &mut bcx,
                    &mut module,
                    &set_ids,
                    t,
                    key_arg,
                    RegKind::Str,
                    val,
                    val_kind,
                );
                guard!(done, i, rop.pc);
            }
            Op::GetField => {
                // sunk path: use_var the virt slot
                // for hash_slot, def_var R[A], propagate kind.
                if let Some(OpAction::GetFieldSunkRead {
                    site_idx,
                    hash_slot,
                }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && let Some(vars) = virt_vars[site_idx as usize].as_ref()
                {
                    let array_cap = escape.sites[site_idx as usize].array_cap as usize;
                    let slot = array_cap + hash_slot as usize;
                    let v = bcx.use_var(vars[slot]);
                    bcx.def_var(regs[ins.a() as usize], v);
                    let k = virt_kinds[site_idx as usize]
                        .as_ref()
                        .expect("Sinkable site has virt_kinds")[slot];
                    current_kinds[off + ins.a() as usize] = k;
                    continue;
                }
                // helper path.
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.b()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.b() as usize]);
                let key_v = match head_proto.consts[ins.c() as usize] {
                    luna_core::runtime::Value::Str(s) => s,
                    _ => unreachable!("pre-emit gates Str const at K[C]"),
                };
                let key_arg =
                    emit_str_key_arg(module, &mut bcx, key_v, opts.aot, &mut defined_aot_data);
                let inferred = if i + 1 < effective_end {
                    infer_getx_exit_lookahead(ins.a(), &record.ops[i + 1..effective_end])
                } else {
                    None
                };
                let want = getx_want(inferred);

                // table-field IC scaffold.
                //
                // When `LUNA_JIT_FIELD_IC=1` and this op is the
                // recorder-captured snapshot site, emit an inline
                // cache: 4 guards (mt None, nodes.len() == cached,
                // node[slot].key.raw == cached_key_bits,
                // node[slot].val.tag == cached_val_tag) + 1 load
                // of node[slot].val.raw. Guard miss falls through
                // to the existing helper-call path so no new deopt
                // edge is introduced (scaffold-safe rollout).
                //
                // env-OFF default short-circuits on the cached
                // atomic load inside `field_ic_enabled()`; the IC
                // emission produces zero additional IR when the
                // gate is off.
                // The IC's tag guard only stands in for the checked read
                // when the cached tag is the one the trace types it as.
                let ic_active = luna_core::jit::trace_types::field_ic_enabled()
                    && record.field_ic_snapshot.as_ref().is_some_and(|s| {
                        s.op_idx as usize == i
                            && want.is_none_or(|(k, _)| {
                                use luna_core::runtime::value::tag;
                                let enum_tag = match k {
                                    RegKind::Int => tag::INT,
                                    RegKind::Float => tag::FLOAT,
                                    _ => tag::TABLE,
                                };
                                enum_tag == s.cached_val_tag
                            })
                    });

                let v = if ic_active {
                    let snap = record
                        .field_ic_snapshot
                        .as_ref()
                        .expect("ic_active implies snapshot present");

                    // --- Guards 1 & 2: metatable + node count ---
                    let mt = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        t,
                        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
                    );
                    let zero = bcx.ins().iconst(types::I64, 0);
                    let mt_ok = bcx.ins().icmp(IntCC::Equal, mt, zero);
                    let node_mask = bcx.ins().load(
                        types::I32,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        t,
                        crate::jit_backend::TABLE_NODE_MASK_OFFSET as i32,
                    );
                    let mask = i64::from((snap.nodes_len as u32).wrapping_sub(1));
                    let len_ok = bcx.ins().icmp_imm_u(IntCC::Equal, node_mask, mask);
                    let guards_12 = bcx.ins().band(mt_ok, len_ok);

                    // 3 blocks: fast (guards 3+4 + load), slow
                    // (helper), merge (def_var dst). slow_blk has 2
                    // predecessors (mt/len fail + key/tag fail); we
                    // seal it only after both edges are emitted.
                    let fast_blk = bcx.create_block();
                    let slow_blk = bcx.create_block();
                    let merge_blk = bcx.create_block();
                    bcx.append_block_param(merge_blk, types::I64);

                    bcx.ins().brif(guards_12, fast_blk, &[], slow_blk, &[]);

                    // --- fast: load nodes_ptr, compute node_addr,
                    //     guards 3 & 4, load val_raw ---
                    bcx.switch_to_block(fast_blk);
                    bcx.seal_block(fast_blk);
                    let nodes_ptr = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        t,
                        crate::jit_backend::TABLE_NODES_PTR_OFFSET as i32,
                    );
                    let node_offset = (snap.slot_idx as usize * crate::jit_backend::SIZEOF_NODE) as i64;
                    let node_addr = bcx.ins().iadd_imm_u(nodes_ptr, node_offset);

                    let key_raw = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        node_addr,
                        crate::jit_backend::NODE_KEY_RAW_OFFSET as i32,
                    );
                    let key_imm = bcx.ins().iconst(types::I64, snap.key_ptr_bits as i64);
                    let key_ok = bcx.ins().icmp(IntCC::Equal, key_raw, key_imm);

                    let val_tag_i8 = bcx.ins().load(
                        types::I8,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        node_addr,
                        crate::jit_backend::NODE_VAL_TAG_OFFSET as i32,
                    );
                    let val_tag = bcx.ins().uextend(types::I64, val_tag_i8);
                    let tag_imm = bcx.ins().iconst(types::I64, snap.cached_val_tag as i64);
                    let tag_ok = bcx.ins().icmp(IntCC::Equal, val_tag, tag_imm);
                    let guards_34 = bcx.ins().band(key_ok, tag_ok);

                    let load_blk = bcx.create_block();
                    bcx.ins().brif(guards_34, load_blk, &[], slow_blk, &[]);

                    bcx.switch_to_block(load_blk);
                    bcx.seal_block(load_blk);
                    let val_raw = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        node_addr,
                        crate::jit_backend::NODE_VAL_RAW_OFFSET as i32,
                    );
                    bcx.ins().jump(merge_blk, &[val_raw.into()]);

                    // --- slow: fall back to the helper ---
                    bcx.switch_to_block(slow_blk);
                    bcx.seal_block(slow_blk);
                    let v_slow = if let Some((_, w)) = want {
                        checked_read!(get_field_checked_id, t, key_arg, w, rop.pc, i)
                    } else {
                        let func_ref = module.declare_func_in_func(get_field_id, bcx.func);
                        let call = bcx.ins().call(func_ref, &[t, key_arg]);
                        bcx.inst_results(call)[0]
                    };
                    bcx.ins().jump(merge_blk, &[v_slow.into()]);

                    // --- merge ---
                    bcx.switch_to_block(merge_blk);
                    bcx.seal_block(merge_blk);
                    bcx.block_params(merge_blk)[0]
                } else if let Some((_, w)) = want {
                    checked_read!(get_field_checked_id, t, key_arg, w, rop.pc, i)
                } else {
                    let func_ref = module.declare_func_in_func(get_field_id, bcx.func);
                    let call = bcx.ins().call(func_ref, &[t, key_arg]);
                    bcx.inst_results(call)[0]
                };
                bcx.def_var(regs[ins.a() as usize], v);

                match inferred {
                    Some(ExitTag::Int) => current_kinds[off + ins.a() as usize] = RegKind::Int,
                    Some(ExitTag::Table) => current_kinds[off + ins.a() as usize] = RegKind::Table,
                    Some(ExitTag::Float) => current_kinds[off + ins.a() as usize] = RegKind::Float,
                    _ => {
                        // as for GetI
                        current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                        dispatchable = false;
                        dispatch_off_reason =
                            dispatch_off_reason.or(Some("GetField:inference-fail"));
                    }
                }
            }
            Op::GetTabUp => {
                // `R[A] := upvals[B][K[C]:string]`.
                // Helper path mirrors GetField's; the sunk-table
                // optimization does NOT apply (upvalue tables are
                // the global env, not trace-internal alloc). Exit-tag
                // inference identical to GetField — peek next op via
                // `infer_getx_exit`.
                let upval_idx_arg = bcx.ins().iconst(types::I64, ins.b() as i64);
                let key_v = match head_proto.consts[ins.c() as usize] {
                    luna_core::runtime::Value::Str(s) => s,
                    _ => unreachable!("pre-emit gates Str const at K[C]"),
                };
                let key_arg =
                    emit_str_key_arg(module, &mut bcx, key_v, opts.aot, &mut defined_aot_data);
                let inferred = if i + 1 < effective_end {
                    infer_getx_exit_lookahead(ins.a(), &record.ops[i + 1..effective_end])
                } else {
                    None
                };
                let v = if let Some((_, w)) = getx_want(inferred) {
                    checked_read!(get_tab_up_checked_id, upval_idx_arg, key_arg, w, rop.pc, i)
                } else {
                    let func_ref = module.declare_func_in_func(get_tab_up_id, bcx.func);
                    let call = bcx.ins().call(func_ref, &[upval_idx_arg, key_arg]);
                    bcx.inst_results(call)[0]
                };
                bcx.def_var(regs[ins.a() as usize], v);
                match inferred {
                    Some(ExitTag::Int) => current_kinds[off + ins.a() as usize] = RegKind::Int,
                    Some(ExitTag::Table) => current_kinds[off + ins.a() as usize] = RegKind::Table,
                    Some(ExitTag::Float) => current_kinds[off + ins.a() as usize] = RegKind::Float,
                    _ => {
                        // as for GetI
                        current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                        dispatchable = false;
                        dispatch_off_reason =
                            dispatch_off_reason.or(Some("GetTabUp:inference-fail"));
                    }
                }
            }
            Op::SetI => {
                // sunk path: when escape sweep tagged
                // SetISunkWrite, def_var the source register into
                // the matching virt slot Variable + propagate the
                // source RegKind into virt_kinds so the next
                // GetIRead restores the right kind into current_kinds.
                if let Some(OpAction::SetISunkWrite { site_idx, key }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && virt_vars[site_idx as usize].is_some()
                {
                    let slot = (key as usize) - 1;
                    let src_kind = current_kinds[off + ins.c() as usize];
                    let v = bcx.use_var(regs[ins.c() as usize]);
                    let vars = virt_vars[site_idx as usize]
                        .as_ref()
                        .expect("Sinkable site has virt_vars");
                    bcx.def_var(vars[slot], v);
                    let kinds_vec = virt_kinds[site_idx as usize]
                        .as_mut()
                        .expect("Sinkable site has virt_kinds");
                    kinds_vec[slot] = src_kind;
                    continue;
                }
                // R[A][B_imm] := R[C] helper path. Dispatch by R[C]
                // kind via emit_table_set (Nil / Int / Closure / etc.).
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.a()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.a() as usize]);
                let k_imm = bcx.ins().iconst(types::I64, ins.b() as i64);
                let val_kind = k_op(&current_kinds, off as u32 + ins.c());
                // a value of unknown kind cannot be tagged for the table
                if val_kind.untyped() {
                    return None;
                }
                let val = bcx.use_var(regs[ins.c() as usize]);
                let done = emit_table_set(
                    &mut bcx,
                    &mut module,
                    &set_ids,
                    t,
                    k_imm,
                    RegKind::Int,
                    val,
                    val_kind,
                );
                guard!(done, i, rop.pc);
            }
            Op::SetTable => {
                // sunk path: escape sweep tagged
                // SetTableSunkWrite when the key reg was const-folded
                // to a 1..=cap literal. Emit shape mirrors SetI sunk
                // (def_var virt slot + propagate kind into virt_kinds).
                if let Some(OpAction::SetTableSunkWrite { site_idx, key }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && virt_vars[site_idx as usize].is_some()
                {
                    let slot = (key as usize) - 1;
                    let src_kind = current_kinds[off + ins.c() as usize];
                    let v = bcx.use_var(regs[ins.c() as usize]);
                    let vars = virt_vars[site_idx as usize]
                        .as_ref()
                        .expect("Sinkable site has virt_vars");
                    bcx.def_var(vars[slot], v);
                    let kinds_vec = virt_kinds[site_idx as usize]
                        .as_mut()
                        .expect("Sinkable site has virt_kinds");
                    kinds_vec[slot] = src_kind;
                    continue;
                }
                // R[A][R[B]] := R[C] helper path. Same kind-dispatch
                // as Op::SetI.
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.a()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.a() as usize]);
                let key = bcx.use_var(regs[ins.b() as usize]);
                let key_kind = k_op(&current_kinds, off as u32 + ins.b());
                let val_kind = k_op(&current_kinds, off as u32 + ins.c());
                // a key or value of unknown kind cannot be tagged for the table
                if key_kind.untyped() || val_kind.untyped() {
                    return None;
                }
                let val = bcx.use_var(regs[ins.c() as usize]);
                let done = emit_table_set(
                    &mut bcx,
                    &mut module,
                    &set_ids,
                    t,
                    key,
                    key_kind,
                    val,
                    val_kind,
                );
                guard!(done, i, rop.pc);
            }
            Op::SetList => {
                // `R[A][C+i] := R[A+i]` for i in
                // 1..=effective_b. effective_b = bytecode B if B>0,
                // else recorder's var_count snapshot (top - A - 1
                // at the op). For sunk path, effective_b == cap
                // (validated in escape sweep).
                let b_bytecode = ins.b() as usize;
                let effective_b = if b_bytecode == 0 {
                    // Unreachable on the sunk path (the escape sweep
                    // already mark_escaped on None). The helper path
                    // bails compile too — None means no live top.
                    record.ops[i].var_count? as usize
                } else {
                    b_bytecode
                };
                if let Some(OpAction::SetListWrite { site_idx }) = escape.op_actions[i]
                    && escape.sites[site_idx as usize].state == EscapeState::Sinkable
                    && virt_vars[site_idx as usize].is_some()
                {
                    let a = ins.a() as usize;
                    let mut src_vals: Vec<Value> = Vec::with_capacity(effective_b);
                    let mut src_kinds: Vec<RegKind> = Vec::with_capacity(effective_b);
                    for vi in 1..=effective_b {
                        src_vals.push(bcx.use_var(regs[a + vi]));
                        src_kinds.push(current_kinds[off + a + vi]);
                    }
                    let vars = virt_vars[site_idx as usize]
                        .as_ref()
                        .expect("Sinkable site has virt_vars");
                    for (vi, &v) in src_vals.iter().enumerate() {
                        bcx.def_var(vars[vi], v);
                    }
                    let kinds_vec = virt_kinds[site_idx as usize]
                        .as_mut()
                        .expect("Sinkable site has virt_kinds");
                    for (vi, &k) in src_kinds.iter().enumerate() {
                        kinds_vec[vi] = k;
                    }
                    continue;
                }
                // Helper path: same loop with effective_b iters.
                let a = ins.a() as usize;
                let c_off = ins.c() as i64;
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + a as u32) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[a]);
                for ii in 1..=effective_b {
                    let key = bcx.ins().iconst(types::I64, c_off + ii as i64);
                    let src_kind = k_op(&current_kinds, (off + a + ii) as u32);
                    // a value of unknown kind cannot be tagged for the table
                    if src_kind.untyped() {
                        return None;
                    }
                    let val = bcx.use_var(regs[a + ii]);
                    // Always stored: SetList fills the fresh table of a
                    // constructor, which has no metatable, at integer keys.
                    let _ = emit_table_set(
                        &mut bcx,
                        &mut module,
                        &set_ids,
                        t,
                        key,
                        RegKind::Int,
                        val,
                        src_kind,
                    );
                }
            }
            Op::Len => {
                // R[A] := #R[B] — call luna_jit_table_len(t) -> i64.
                // the helpers read the operand as a table. A number,
                // string or closure (entry-guarded or computed here) leaves
                // the op to the interpreter; Nil can be a lookahead guess
                // for a value the recording indexed, so it stays
                match k_op(&current_kinds, off as u32 + ins.b()) {
                    RegKind::Table | RegKind::Nil => {}
                    RegKind::Unset | RegKind::Unknown => {
                        dispatchable = false;
                        dispatch_off_reason = dispatch_off_reason.or(Some("table-op:unknown-kind"));
                    }
                    _ => return None,
                }
                let t = bcx.use_var(regs[ins.b() as usize]);
                let func_ref = module.declare_func_in_func(len_checked_id, bcx.func);
                let call = bcx.ins().call(func_ref, &[t]);
                let v = bcx.inst_results(call)[0];
                // -1: the table has a metatable
                let ok = bcx.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, v, 0);
                guard!(ok, i, rop.pc);
                bcx.def_var(regs[ins.a() as usize], v);
                current_kinds[off + ins.a() as usize] = RegKind::Int;
            }
            Op::Closure => {
                // R[A] := closure(proto.protos[Bx]).
                // Emit per-in_stack-upval spill followed by a
                // single op_closure helper call. Spill writes
                // vm.stack[base + d.index] = Value::pack(tag, raw)
                // so the helper's find_or_create_upval captures a
                // live slot. Restrictions enforced in pre-emit:
                // inline_depth == 0 + every in_stack source reg in
                // bounds. RegKind::Unset src → bail (no known tag).
                let bx = ins.bx() as usize;
                let inner = head_proto.protos[bx];
                let spill_ref = module.declare_func_in_func(spill_id, bcx.func);
                for d in inner.upvals.iter() {
                    if !d.in_stack {
                        continue;
                    }
                    let src_idx = d.index as usize;
                    let src_kind = current_kinds[off + src_idx];
                    // an untyped source cannot be packed to a Value
                    let tag_byte = known_tag(src_kind)?;
                    let slot_arg = bcx.ins().iconst(types::I64, d.index as i64);
                    let tag_arg = bcx.ins().iconst(types::I64, tag_byte as i64);
                    let raw_arg = bcx.use_var(regs[src_idx]);
                    bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
                }
                let bx_arg = bcx.ins().iconst(types::I64, ins.bx() as i64);
                let func_ref = module.declare_func_in_func(op_closure_id, bcx.func);
                let call = bcx.ins().call(func_ref, &[bx_arg]);
                let v = bcx.inst_results(call)[0];
                bcx.def_var(regs[ins.a() as usize], v);
                current_kinds[off + ins.a() as usize] = RegKind::Closure;
                closure_seen += 1;
            }
            Op::Close => {
                // close open upvals at slot ≥ A.
                //
                // Sequence:
                //  1. Pre-Close spill every slot in [A..max_stack)
                //     whose current_kinds is known (helper's close_from
                //     reads vm.stack[s] to seal each upval, so the
                //     trace's IR Variable values must reach vm.stack
                //     first).
                //  2. Call `luna_jit_op_close(A)` → 0 (continue) or
                //     1 (deopt: handler would run / pre-pending_err).
                //  3. brif on the i64 status: continue_blk falls
                //     through to subsequent ops; deopt_blk does a
                //     full store_back of all regs and returns close_pc
                //     so the interpreter redoes the Op::Close cleanly.
                //
                // close_from is idempotent (open_upvals are popped on
                // first call), so a deopt that re-fires interp's
                // Op::Close → begin_close → close_from sees no work.
                let a_us = ins.a() as usize;
                let spill_ref = module.declare_func_in_func(spill_id, bcx.func);
                for slot in a_us..max_stack {
                    let k = current_kinds[off + slot];
                    let Some(tag_byte) = known_tag(k) else {
                        continue;
                    };
                    let slot_arg = bcx.ins().iconst(types::I64, slot as i64);
                    let tag_arg = bcx.ins().iconst(types::I64, tag_byte as i64);
                    let raw_arg = bcx.use_var(regs[slot]);
                    bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
                }
                let a_arg = bcx.ins().iconst(types::I64, ins.a() as i64);
                let func_ref = module.declare_func_in_func(op_close_id, bcx.func);
                let call = bcx.ins().call(func_ref, &[a_arg]);
                let status = bcx.inst_results(call)[0];
                // 1: a `__close` handler would run; the interpreter
                // redoes the op and runs it
                let ok = bcx.ins().icmp_imm_s(IntCC::Equal, status, 0);
                guard!(ok, i, rop.pc);
            }
            Op::GetUpval => {
                // R[A] := UpVal[B]. The helper reads JIT_CL's
                // upvals[B] and returns the raw 8-byte payload.
                // use-site inference (`infer_upval_exit`)
                // pins the kind when the immediate use is `Op::Call`
                // on R[A] (the call target must be a closure). Any
                // other shape leaves dispatchable=false. Per-side-exit
                // exit_tags guard side-exits firing
                // BEFORE this GetUpval: they snapshot the pre-GetUpval
                // current_kinds, so those exits restore as Untouched.
                //
                // memoize per upval idx via `upval_cache`.
                let idx_b = ins.b();
                let v = if let Some(&cached_var) = upval_cache.get(&idx_b) {
                    bcx.use_var(cached_var)
                } else {
                    let idx_arg = bcx.ins().iconst(types::I64, ins.b() as i64);
                    let func_ref = module.declare_func_in_func(upval_get_id, bcx.func);
                    let call = bcx.ins().call(func_ref, &[idx_arg]);
                    let new_v = bcx.inst_results(call)[0];
                    let cache_var = bcx.declare_var(types::I64);
                    bcx.def_var(cache_var, new_v);
                    upval_cache.insert(idx_b, cache_var);
                    new_v
                };
                bcx.def_var(regs[ins.a() as usize], v);
                // Look forward including the terminator (effective_end
                // is the Op::Call's index when truncation applies).
                let upper = effective_end.min(record.ops.len() - 1) + 1;
                let inferred = if i + 1 < upper {
                    infer_upval_exit(ins.a(), &record.ops[i + 1..upper])
                } else {
                    None
                };
                match inferred {
                    Some(ExitTag::Closure) => {
                        current_kinds[off + ins.a() as usize] = RegKind::Closure;
                    }
                    _ => {
                        current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                        dispatchable = false;
                        dispatch_off_reason =
                            dispatch_off_reason.or(Some("GetUpval:not-Closure-use"));
                    }
                }
            }
            // inline self-recursive Call: emit nothing.
            // The recorder's depth bump (next op at depth+1) drives the
            // op_offsets shift; subsequent emit lands in the callee's
            // register window via the `off` shadow.
            //
            // push the callee frame onto `call_chain`
            // so subsequent cmp@d>0 sites can snapshot the chain. The
            // pushed `pc` is the caller's resume PC (Call.pc + 1); the
            // innermost frame's pc is overwritten with the side-exit PC
            // at snapshot time.
            Op::Call => {
                // The inlined body is the head proto's code run with the
                // entry closure's upvalues, which is right only when the
                // callee is that very closure. Anything else (another
                // closure of the proto, a reassigned upvalue, another
                // function) leaves here and the interpreter makes the call.
                let callee_reg = ins.a() as usize;
                if !matches!(current_kinds[off + callee_reg], RegKind::Closure) {
                    checkpoint("bail:inline-callee-not-closure");
                    return None;
                }
                let head_cl = match head_closure_var {
                    Some(var) => bcx.use_var(var),
                    None => {
                        let func_ref = module.declare_func_in_func(head_closure_id, bcx.func);
                        let call = bcx.ins().call(func_ref, &[]);
                        let v = bcx.inst_results(call)[0];
                        let var = bcx.declare_var(types::I64);
                        bcx.def_var(var, v);
                        head_closure_var = Some(var);
                        v
                    }
                };
                let callee = bcx.use_var(regs[callee_reg]);
                let same = bcx.ins().icmp(IntCC::Equal, callee, head_cl);
                guard!(same, i, rop.pc);
                // SelfLink close: the LAST recorded op is the
                // Op::Call whose "next" op (the tripping deepest-depth
                // entry) was never captured. Skip the call_chain push
                // for that trailing Call — the SelfLink tail emit
                // computes its bump_off from this Call's offset + A + 1
                // directly. No FrameMaterializeInfo needed because no
                // side-exit can fire inside the tripping callee (it has
                // no recorded body).
                if self_link_idx_opt.is_some() && i + 1 == effective_end {
                    continue;
                }
                // Next op is at depth+1 (recorder invariant for
                // self-recursive entry); its op_offsets entry is the
                // callee's base_offset.
                debug_assert!(
                    i + 1 < effective_end,
                    "self-rec Call must be followed by callee op in effective_end"
                );
                let callee_base = op_offsets[i + 1];
                call_chain.push(FrameMaterializeInfo {
                    base_offset: callee_base,
                    pc: rop.pc + 1,
                    nresults: 1,
                });
            }
            // inline Return0: callee returns no values
            // back to the caller. The caller's R[call_a..] slots stay
            // whatever the caller had written (Lua semantics: the
            // return values are nil if the caller's call expected
            // more than the callee delivered; here recorder snapshots
            // a single concrete trip so trust the recorded trace).
            // pop the matching call_chain frame.
            Op::Return0 => {
                debug_assert!(
                    !call_chain.is_empty(),
                    "Return0 at depth>0 has a matching frame"
                );
                call_chain.pop();
            }
            // inline Return1: copy callee's R[A]
            // into the caller's R[call_a]. `op_offsets` for the
            // following ops will revert to the caller's window, but
            // the value lives in `regs_full[caller_off + call_a]`
            // ready for the caller's continuation to read it.
            Op::Return1 => {
                let a_callee = ins.a() as usize;
                let call_a = enclosing_call_a[i]
                    .expect("Return1 at depth>0 has an enclosing Op::Call")
                    as usize;
                // Caller window's offset is below ours by call_a+1
                // (callee R[0] sits at caller R[call_a+1]).
                let caller_off = off
                    .checked_sub(call_a + 1)
                    .expect("op_offsets invariant: callee window > caller window");
                let src_var = regs_full[off + a_callee];
                let dst_var = regs_full[caller_off + call_a];
                let v = bcx.use_var(src_var);
                bcx.def_var(dst_var, v);
                // Propagate the kind so the caller's continuation
                // sees the right type.
                current_kinds[caller_off + call_a] = current_kinds[off + a_callee];
                // pop matching call_chain frame.
                debug_assert!(
                    !call_chain.is_empty(),
                    "Return1 at depth>0 has a matching frame"
                );
                call_chain.pop();
            }
            // generic-for body tail. Sequence:
            //   1. Spill regs[A..=A+2] (iter / state / control) to
            //      vm.stack so the helper's
            //      `vm.stack[A+4..=A+6] = vm.stack[A..=A+2]` copy
            //      sees current trace values (control changes each
            //      iter via TForLoop's R[A+2] = R[A+4] writeback).
            //   2. Call `luna_jit_op_tforcall(A, nvars)`. Status
            //      `< 0` → deopt (Lua-closure iter or runtime err).
            //   3. Continue branch: reload regs[A+2] + regs[A+4..]
            //      from vm.stack so subsequent body iters (after the
            //      back-edge from TForLoop) see iter results.
            //      current_kinds for reloaded slots = Unset; the
            //      first body iter still uses entry-tag kinds, and
            //      TForLoop tail's tag-check guards the back-edge
            //      so runtime types match emit-time assumptions.
            Op::TForCall => {
                let a_us = ins.a() as usize;
                let nvars = ins.c() as i64;
                // ipairs detection. Recorder's TForLoop
                // trigger snapshots `R[A]` if Native; we compare against
                // `ipairs_iter`'s address to specialise emit into inline
                // Table aget IR (skip the `op_tforcall` C call entirely
                // on the hot path).
                let ipairs_addr = luna_core::vm::builtins::ipairs_iter
                    as luna_core::runtime::value::NativeFn
                    as usize;
                let is_ipairs_trace = record.tfor_iter_ptr == Some(ipairs_addr);

                // spill discipline:
                // - non-ipairs case: spill R[A..=A+2] upfront (helper
                //   path runs unconditionally; needs vm.stack populated).
                // - ipairs case: SKIP the upfront spill on the hot
                //   path (R[A] and R[A+1] never change inside the
                //   trace — vm.stack still holds entry values, which
                //   is what the slow_blk helper reads). R[A+2] is
                //   spilled INSIDE slow_blk only, so fast iters pay
                //   nothing.
                let spill_ref = module.declare_func_in_func(spill_id, bcx.func);
                let spill_slot = |bcx: &mut FunctionBuilder<'_>, slot: usize| {
                    let k = current_kinds[off + slot];
                    let Some(tag_byte) = known_tag(k) else {
                        return;
                    };
                    let slot_arg = bcx.ins().iconst(types::I64, slot as i64);
                    let tag_arg = bcx.ins().iconst(types::I64, tag_byte as i64);
                    let raw_arg = bcx.use_var(regs[slot]);
                    bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
                };
                if !is_ipairs_trace {
                    for slot in a_us..=(a_us + 2) {
                        spill_slot(&mut bcx, slot);
                    }
                }

                // The helper-call path (used by slow_blk in the
                // ipairs case + the non-ipairs case wholesale).
                // Allocates the 3-slot buffer, calls the helper,
                // brif-checks the result, def_vars regs + tag from
                // the buffer.
                macro_rules! emit_helper_call {
                    () => {{
                        let out_ss =
                            bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                                24,
                                3,
                            ));
                        let ctrl_addr = bcx.ins().stack_addr(types::I64, out_ss, 0);
                        let key_addr = bcx.ins().stack_addr(types::I64, out_ss, 8);
                        let val_addr = bcx.ins().stack_addr(types::I64, out_ss, 16);
                        let a_arg = bcx.ins().iconst(types::I64, a_us as i64);
                        let nvars_arg = bcx.ins().iconst(types::I64, nvars);
                        let func_ref = module.declare_func_in_func(op_tforcall_id, bcx.func);
                        let call_inst = bcx
                            .ins()
                            .call(func_ref, &[a_arg, nvars_arg, ctrl_addr, key_addr, val_addr]);
                        let status_or_tag = bcx.inst_results(call_inst)[0];
                        // -1: not a native iterator, or it raised; the
                        // interpreter redoes the op
                        let ok =
                            bcx.ins()
                                .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, status_or_tag, 0);
                        guard!(ok, i, rop.pc);
                        // key tag | value tag << 8 (Vm::jit_op_tforcall)
                        let key_tag = bcx.ins().band_imm_u(status_or_tag, 0xff);
                        let val_tag = bcx.ins().ushr_imm_u(status_or_tag, 8);
                        bcx.def_var(tforcall_tag_var, key_tag);
                        bcx.def_var(tforcall_val_tag_var, val_tag);
                        let ctrl_raw = bcx.ins().stack_load(types::I64, types::I64, out_ss, 0);
                        let key_raw = bcx.ins().stack_load(types::I64, types::I64, out_ss, 8);
                        let val_raw = bcx.ins().stack_load(types::I64, types::I64, out_ss, 16);
                        bcx.def_var(regs[a_us + 2], ctrl_raw);
                        bcx.def_var(regs[a_us + 4], key_raw);
                        if (nvars as usize) >= 2 && a_us + 5 < max_stack {
                            bcx.def_var(regs[a_us + 5], val_raw);
                        }
                    }};
                }

                if is_ipairs_trace {
                    // Inline aget fast path. The recorder confirmed
                    // R[A] = ipairs_iter at trace start. The standard
                    // ipairs loop has R[A+1] = Table (state) and
                    // R[A+2] = Int (control = last seen index).
                    // Per iter: next_i = ctrl + 1; val = t[next_i].
                    // If val is Nil → loop ends; else key = next_i,
                    // val_raw = val's payload.
                    let ctrl = bcx.use_var(regs[a_us + 2]);
                    let t_raw = bcx.use_var(regs[a_us + 1]);
                    let one = bcx.ins().iconst(types::I64, 1);
                    let next_i = bcx.ins().iadd(ctrl, one);
                    let key_m1 = ctrl;

                    let asize = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        t_raw,
                        crate::jit_backend::TABLE_ASIZE_OFFSET as i32,
                    );
                    let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_m1, asize);
                    let metatable = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        t_raw,
                        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
                    );
                    let zero = bcx.ins().iconst(types::I64, 0);
                    let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero);
                    let fast_ok = bcx.ins().band(in_range, no_meta);

                    let fast_blk = bcx.create_block();
                    let slow_blk = bcx.create_block();
                    let merge_blk = bcx.create_block();
                    bcx.ins().brif(fast_ok, fast_blk, &[], slow_blk, &[]);

                    // ----- fast_blk: inline aget + populate -----
                    bcx.switch_to_block(fast_blk);
                    bcx.seal_block(fast_blk);
                    let avals_ptr = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        t_raw,
                        crate::jit_backend::TABLE_ARRAY_PTR_OFFSET as i32,
                    );
                    let three = bcx.ins().iconst(types::I64, 3);
                    let val_off = bcx.ins().ishl(key_m1, three);
                    let val_addr_fast = bcx.ins().iadd(avals_ptr, val_off);
                    let val_raw_fast = bcx.ins().load(
                        types::I64,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        val_addr_fast,
                        0,
                    );
                    let avals_bytes = bcx.ins().ishl(asize, three);
                    let tag_base = bcx.ins().iadd(avals_ptr, avals_bytes);
                    let tag_addr = bcx.ins().iadd(tag_base, key_m1);
                    let val_tag_i8 = bcx.ins().load(
                        types::I8,
                        cranelift_codegen::ir::MemFlagsData::trusted(),
                        tag_addr,
                        0,
                    );
                    let val_tag = bcx.ins().uextend(types::I64, val_tag_i8);
                    let nil_const = bcx
                        .ins()
                        .iconst(types::I64, luna_core::runtime::value::raw::NIL as i64);
                    let int_const = bcx
                        .ins()
                        .iconst(types::I64, luna_core::runtime::value::raw::INT as i64);
                    let is_nil = bcx.ins().icmp(IntCC::Equal, val_tag, nil_const);
                    // runtime val_tag guard. Snapshot
                    // at recorder fire (R[A+5]'s tag) is the
                    // *expected* iter val tag. The trace's
                    // downstream emit (Move propagation, Concat
                    // spill via RegKind::Str etc.) is specialised
                    // to this tag. If a subsequent iter delivers a
                    // different non-Nil tag (mixed-tag array), the
                    // spill would pack stale bits as the snapshot
                    // tag → garbage Value. Guard: `val_tag == Nil
                    // OR val_tag == expected_tag` → continue, else
                    // deopt. Skip the guard when no snapshot is
                    // available (snapshot=None) or when the
                    // snapshot is Nil itself (degenerate).
                    if let Some(expected_tag) = record.tfor_val_tag
                        && expected_tag != luna_core::runtime::value::raw::NIL
                    {
                        let exp_const = bcx.ins().iconst(types::I64, expected_tag as i64);
                        let is_exp = bcx.ins().icmp(IntCC::Equal, val_tag, exp_const);
                        let ok = bcx.ins().bor(is_nil, is_exp);
                        let guard_continue = bcx.create_block();
                        let guard_deopt = bcx.create_block();
                        bcx.ins().brif(ok, guard_continue, &[], guard_deopt, &[]);
                        bcx.switch_to_block(guard_deopt);
                        bcx.seal_block(guard_deopt);
                        // restored with the kinds the registers have here
                        guard_exit!(rop.pc, i);
                        bcx.switch_to_block(guard_continue);
                        bcx.seal_block(guard_continue);
                    }
                    let zero_raw = bcx.ins().iconst(types::I64, 0);
                    // R[A+4] = is_nil ? Nil(raw=0) : Int(raw=next_i)
                    let r4_raw = bcx.ins().select(is_nil, zero_raw, next_i);
                    let r4_tag = bcx.ins().select(is_nil, nil_const, int_const);
                    bcx.def_var(regs[a_us + 2], next_i);
                    bcx.def_var(regs[a_us + 4], r4_raw);
                    if a_us + 5 < max_stack {
                        // On the Nil branch, exit_tag[A+5] stays
                        // `Untouched` (no per-side-exit override
                        // for A+5), so the dispatcher restores
                        // using entry_tag. If entry was a Str
                        // slot, packing with raw=0 produces a null
                        // Gc<LuaStr> → panic on the next interp
                        // touch. Preserve the previous regs[A+5]
                        // (= the last non-Nil iter's value) on the
                        // Nil branch so the trace exit restore
                        // sees a real GC pointer.
                        let prev_v5 = bcx.use_var(regs[a_us + 5]);
                        let chosen_v5 = bcx.ins().select(is_nil, prev_v5, val_raw_fast);
                        bcx.def_var(regs[a_us + 5], chosen_v5);
                    }
                    bcx.def_var(tforcall_tag_var, r4_tag);
                    bcx.def_var(tforcall_val_tag_var, val_tag);
                    bcx.ins().jump(merge_blk, &[]);

                    // ----- slow_blk: helper fallback -----
                    bcx.switch_to_block(slow_blk);
                    bcx.seal_block(slow_blk);
                    // Spill R[A+2] (ctrl, the only slot that
                    // changes per iter via TForLoop's writeback)
                    // so the helper sees the trace's current
                    // value. R[A]/R[A+1] still hold their entry
                    // values in vm.stack.
                    spill_slot(&mut bcx, a_us + 2);
                    emit_helper_call!();
                    bcx.ins().jump(merge_blk, &[]);

                    // ----- merge_blk -----
                    bcx.switch_to_block(merge_blk);
                    bcx.seal_block(merge_blk);
                } else {
                    emit_helper_call!();
                }

                current_kinds[off + a_us + 2] = RegKind::Unknown;
                current_kinds[off + a_us + 4] = RegKind::Unknown;
                if (nvars as usize) >= 2 && a_us + 5 < max_stack {
                    current_kinds[off + a_us + 5] = RegKind::Unknown;
                }
            }
            // N-operand concat via helper.
            Op::Concat => {
                let a_us = ins.a() as usize;
                let n_operands = ins.b() as usize;
                // Spill every operand slot to vm.stack so the
                // helper's concat_run can read them. For Unset
                // kinds (e.g. Str — RegKind doesn't carry Str)
                // call stack_update_raw which preserves the
                // existing tag and only refreshes the raw bits.
                let spill_ref = module.declare_func_in_func(spill_id, bcx.func);
                let update_raw_ref = module.declare_func_in_func(update_raw_id, bcx.func);
                for slot in a_us..(a_us + n_operands) {
                    let k = current_kinds[off + slot];
                    let slot_arg = bcx.ins().iconst(types::I64, slot as i64);
                    let raw_arg = bcx.use_var(regs[slot]);
                    let tag_byte_opt = match k {
                        // an operand is read, so never held on the stack
                        RegKind::StackHeld => return None,
                        k => known_tag(k),
                    };
                    if let Some(tag_byte) = tag_byte_opt {
                        let tag_arg = bcx.ins().iconst(types::I64, tag_byte as i64);
                        bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
                    } else {
                        bcx.ins().call(update_raw_ref, &[slot_arg, raw_arg]);
                    }
                }
                // Call helper.
                let a_arg = bcx.ins().iconst(types::I64, a_us as i64);
                let n_arg = bcx.ins().iconst(types::I64, n_operands as i64);
                let func_ref = module.declare_func_in_func(op_concat_id, bcx.func);
                let call_inst = bcx.ins().call(func_ref, &[a_arg, n_arg]);
                let status = bcx.inst_results(call_inst)[0];
                // -1: an error or `__concat`; the interpreter redoes the op
                let ok = bcx.ins().icmp_imm_s(IntCC::Equal, status, 0);
                guard!(ok, i, rop.pc);
                // Reload regs[A] (= result Str) from vm.stack via
                // luna_jit_stack_load helper. The helper deopts on the
                // `__concat` path, so a result here is always a string.
                let stack_load_ref = module.declare_func_in_func(stack_load_id, bcx.func);
                let a_arg_reload = bcx.ins().iconst(types::I64, a_us as i64);
                let reload_inst = bcx.ins().call(stack_load_ref, &[a_arg_reload]);
                let result_raw = bcx.inst_results(reload_inst)[0];
                bcx.def_var(regs[a_us], result_raw);
                current_kinds[off + a_us] = RegKind::Str;
            }
            // generic-for prep is the leading pc-bump
            // before body_top. Recorder enters at body_top, so this
            // arm is defensive only — pre-emit pass bails before we
            // reach it.
            Op::TForPrep => unreachable!("Op::TForPrep bailed in pre-emit pass"),
            // TForLoop is the trace's terminator; tail
            // emit handles the side-exit + back-edge.
            Op::TForLoop => unreachable!("Op::TForLoop only appears at effective_end"),
            _ => unreachable!("non-whitelisted op rejected in pre-emit pass"),
        }
    }

    // --- tail.
    //
    // Four cases pick the clean-close shape:
    //
    // - Trace truncated by `Op::Call` → store back + return
    //   `call.pc`. The Call's interp re-execution is the
    //   "exit" — no loop possible.
    // - Trace closes on `Op::ForLoop` (5.4+ Int count form) →
    //   emit the count check + step IR, then either jump back
    //   to `body_loop` (continue path) or side-exit at
    //   `forloop.pc + 1` (loop exit). In one-shot mode the
    //   continue path returns `head_pc` instead, so the
    //   dispatcher gets one iter per entry.
    // - `opts.internal_loop && has_cmp` (and no Call truncation,
    //   no ForLoop) → jump back to `body_loop`. The trace runs
    //   natively until some cmp side-exits; the dispatcher's
    //   per-entry marshal cost amortizes across however many
    //   iterations the loop runs.
    // - Otherwise (one-shot mode or a no-cmp trace) → store back
    //   + return `head_pc`. The dispatcher re-enters per
    //   iteration; an internal loop with no side-exit would
    //   spin forever.
    // every tail `emit_store_back_and_return_pc`
    // passes `&regs_full[..max_stack]` so the store-back ONLY writes
    // the caller's window back to interp stack. Slots at
    // [max_stack..window_size) are inline-frame scratch and must not
    // leak into the dispatcher's reg_state restore.
    let caller_regs: &[Variable] = &regs_full[..max_stack];
    // populated by the `downrec_idx_opt` arm when
    // it emits the stitch sentinel. Flows into `CompiledTrace.
    // downrec_link` at the struct literal below. `None` for every
    // other close shape.
    let mut downrec_link_for_compiled: Option<(u32, u32)> = None;
    let mut downrec_multi_way_count_for_compiled: u8 = 0;
    if let Some((_dr_idx, dr_return_pc, _target_proto_id, _depth_delta)) = downrec_idx_opt {
        // `TraceEnd::DownRec` close: emit the
        // stitch-sentinel + caller-pc-guard.
        //
        // Shape mirrors LuaJIT's `asm_retf` (`lj_asm_arm64.h:565`):
        //   1. Load the saved caller PC.
        //   2. CMP against IR-baked candidate caller PCs.
        //   3. brif eq → stitch_blk: return DOWNREC sentinel +
        //      `record.head_pc` so the dispatcher can walk
        //      `downrec_link` + RetfRecord chain to materialise the
        //      inlined frame and tail-call into the stitched child
        //      trace.
        //   4. brif ne → deopt_blk: safe deopt-tail — store
        //      back caller window + return `head_pc` through the
        //      GLOBAL sentinel; the dispatcher resumes interp at
        //      head_pc.
        debug_assert!(
            dr_return_pc != 0,
            "DownRec recorder should never trip on a PC=0 Op::Return — Op::Return's PC is past the prologue"
        );
        let _ = _target_proto_id;
        let _ = _depth_delta;

        let stitch_blk = bcx.create_block();
        let deopt_blk = bcx.create_block();

        // multi-way caller-pc guard. A single CMP
        // (`saved_pc == dr_return_pc`) misses ~90% of the time on a
        // fib(3) hot loop because
        // the typical fib body has TWO call sites at distinct
        // `pc + 1` caller_pcs — only one of them ever matches
        // `dr_return_pc` (the recorder picks the most-recent
        // threshold-tripping one). The recorder's `rec.retfs`
        // side-channel already collected every depth>0 Return's
        // `caller_pc` + `proto`, so the lowerer here can fan the
        // single CMP into a chain of `icmp(Equal, saved_pc, iconst
        // (candidate_pc)) + brif(eq, stitch, next)` predicates and
        // accept any of them as a HIT. Dedupe over `caller_pc`
        // (mirrors LuaJIT `lj_record.c:897 check_downrec_unroll`'s
        // "count IR_RETF entries by op1 == ptref" walk filtered to
        // the close marker's `target_proto`).
        //
        // Saved-PC slot (`reg_state[window_size_us * 8]`) populated
        // by the dispatcher pre-invoke (see `crates/luna-core/src/
        // vm/exec.rs` `is_downrec_entry` block) with the parent
        // (caller) frame's `pc` — the runtime analogue of LuaJIT's
        // `[base-8]` in `asm_retf` (`lj_asm_arm64.h:565`).
        let saved_pc_offset = (window_size_us as i32) * 8;
        let saved_pc = bcx.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            reg_state,
            saved_pc_offset,
        );
        // Collect distinct caller_pcs from retfs whose proto matches
        // the close marker's `_target_proto_id`. Dedupe + bound to
        // `DOWNREC_MULTI_WAY_GUARD_MAX` so IR size stays predictable
        // regardless of how many retfs the recorder captured.
        // `dr_return_pc` (the close marker's most-recent caller_pc) is
        // inserted first so the chain covers the single-CMP shape's
        // baseline even when filtering eliminates it.
        let mut candidates: Vec<u32> = Vec::with_capacity(DOWNREC_MULTI_WAY_GUARD_MAX);
        candidates.push(dr_return_pc);
        for retf in &record.retfs {
            if candidates.len() >= DOWNREC_MULTI_WAY_GUARD_MAX {
                break;
            }
            if retf.proto.as_ptr() as usize == _target_proto_id
                && !candidates.contains(&retf.caller_pc)
            {
                candidates.push(retf.caller_pc);
            }
        }
        // Emit CMP-chain. For each candidate: `icmp(Equal, ...) + brif`.
        // The last candidate's miss arm branches directly to deopt_blk;
        // earlier candidates' miss arms branch into a fresh block that
        // becomes the next CMP's "current block".
        for (i, candidate_pc) in candidates.iter().enumerate() {
            let imm_pc = bcx.ins().iconst(types::I64, *candidate_pc as i64);
            let eq = bcx.ins().icmp(IntCC::Equal, saved_pc, imm_pc);
            let miss_blk = if i + 1 < candidates.len() {
                bcx.create_block()
            } else {
                deopt_blk
            };
            bcx.ins().brif(eq, stitch_blk, &[], miss_blk, &[]);
            if i + 1 < candidates.len() {
                bcx.switch_to_block(miss_blk);
                bcx.seal_block(miss_blk);
            }
        }
        let multi_way_candidate_count = candidates.len();

        // Hit: return DOWNREC sentinel + `record.head_pc` as the
        // low 32 bits. The full encoded value is
        //   raw_ret = (1u64 << 63)             // side-trace marker
        //           | ((DOWNREC_CODE as u64) << 56)
        //           | (record.head_pc as u64)
        // (bit 63 set so the dispatcher's `from_side_trace` branch
        // at `exec.rs:6354+` decodes through the sentinel switch).
        // The dispatcher's stitch arm reads `parent_ct.downrec_link` for the
        // stitch target rather than looking up via `side_trace_cache`.
        bcx.switch_to_block(stitch_blk);
        bcx.seal_block(stitch_blk);
        let raw_ret =
            (1u64 << 63) | ((SIDE_SENT_DOWNREC_CODE as u64) << 56) | (record.head_pc as u64);
        let stitch_ret = bcx.ins().iconst(types::I64, raw_ret as i64);
        bcx.ins().return_(&[stitch_ret]);

        // Miss: safe deopt-tail. The interpreter runs the head op before
        // anything enters the trace again: entered at once with the same
        // registers it would miss the same way forever.
        bcx.switch_to_block(deopt_blk);
        bcx.seal_block(deopt_blk);
        let r = module.declare_func_in_func(suppress_admit_id, bcx.func);
        bcx.ins().call(r, &[]);
        emit_store_back_and_return_pc(
            &mut bcx,
            caller_regs,
            &stored,
            reg_state,
            record.head_pc,
            flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );

        // Populate downrec_link with the placeholder
        // (trace_id=0, target_head_pc=record.head_pc). The
        // `trace_id=0` sentinel means "self-stitch — target is the
        // trace currently dispatching"; the dispatcher interprets
        // this when resolving the stitch target.
        //
        // The dispatcher admits a trace with a link even when it is not
        // dispatchable, so a body already marked (a value of unknown
        // type) gets no link: that mark only ever turns the trace off.
        if dispatchable {
            downrec_link_for_compiled = Some((0, record.head_pc));
        }

        // With at least 2 distinct caller_pc candidates the multi-way
        // guard hits often enough for the primary dispatcher arm to
        // admit the trace, so it stays dispatchable (unless its body
        // was already marked). The single-CMP fallback (count == 1)
        // sets `dispatchable = false` + `"downrec-stitch-pending"`
        // because its ~90% miss-rate would translate to 90% extra
        // deopt cost if the primary dispatcher arm admitted the trace
        // unconditionally; the dispatcher's `is_downrec_entry` arm
        // keys on `ct.downrec_link.is_some()`, so a linked trace is
        // still admitted there. The multi-way count is surfaced via
        // the `multi_way_guard_emitted` counter, bumped at the close
        // handler from `downrec_multi_way_count_for_compiled` below.
        if multi_way_candidate_count < 2 {
            dispatchable = false;
            dispatch_off_reason = dispatch_off_reason.or(Some("downrec-stitch-pending"));
        }
        downrec_multi_way_count_for_compiled =
            multi_way_candidate_count.min(u8::MAX as usize) as u8;
    } else if let Some((_self_link_idx, _kind)) = self_link_idx_opt {
        // Self-link close deopts instead of looping natively.
        //
        // A native tail (slot-copy `regs_full[i] = regs_full[bump_off
        // + i]` for `i in 0..max_stack`, deepest inlined frame → head
        // frame, then `jump(body_loop)`) would mirror LuaJIT's
        // `asm_tail_link` (`lj_asm.c:2131`) only
        // syntactically. LuaJIT's pre-op snapshots distinguish each
        // frame's typed-slot mapping; luna's slot-copy assumes deepest
        // frame layout == head frame layout, which is sound for plain
        // tail-recursion but not for self-recursion through a
        // non-tail-call body (fib: `Lt → branch → Sub Call Sub Call Add
        // Return`, with depth-0 Sub writes polluting head-frame slots
        // BEFORE the recursive Call, plus a depth>0 base-case Return
        // whose deeper frame layout doesn't match head's). With that
        // tail fib(28) returns 45 instead of 317_811.
        //
        // So the tail is a clean deopt: store
        // back the caller window, return `head_pc`, and pin
        // `dispatchable = false`. The trace still compiles (cranelift
        // accepts a valid back-edge-free fn so the body's mcode and
        // window_size extension stay sound) but the dispatcher's
        // pre-invoke `dispatchable` check refuses to enter it, so
        // interp runs the recursion naturally and produces the correct
        // result.
        //
        // The `RetfRecord` side-channel populated by the recorder
        // (exec.rs gate on `self_link_enabled`) captures the
        // inlined-frame topology that the down-rec stitch consumes
        // to guard a real native back-edge.
        //
        // The `window_size_us` extension above (`record.self_link
        // _kind.is_some()` arm) stays intact — body emit still writes
        // depth>0 slots into the extended buffer; the writes are
        // simply dead here. As for the downrec miss, the interpreter runs
        // the head op before the trace is entered again.
        let r = module.declare_func_in_func(suppress_admit_id, bcx.func);
        bcx.ins().call(r, &[]);
        emit_store_back_and_return_pc(
            &mut bcx,
            caller_regs,
            &stored,
            reg_state,
            record.head_pc,
            flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("self-link-retf-r1"));
    } else if let Some(call_idx) = call_idx_opt {
        emit_store_back_and_return_pc(
            &mut bcx,
            caller_regs,
            &stored,
            reg_state,
            record.ops[call_idx].pc,
            flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(inline_abort_idx) = inline_abort_idx_opt {
        // InlineAbort: emit-up-to-i, then store back
        // + return record.ops[i].pc. Dispatchable is forced false
        // below (the interp can't resume at a depth>0 PC without the
        // CallFrames the trace inlined past).
        emit_store_back_and_return_pc(
            &mut bcx,
            caller_regs,
            &stored,
            reg_state,
            record.ops[inline_abort_idx].pc,
            flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(return_idx) = return_idx_opt {
        // Return0/Return1 at depth=0: caller frame
        // unwinds. Same shape as Call truncation — store back caller
        // window + return the Return op's PC so the interp re-executes
        // it with the correct register state. Subject to the same
        // length-gate dispatchable check below.
        emit_store_back_and_return_pc(
            &mut bcx,
            caller_regs,
            &stored,
            reg_state,
            record.ops[return_idx].pc,
            flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(for_loop_idx) = for_loop_idx_opt {
        // 5.4+ Int count form (validated above; pre53 bails).
        //
        //   if R[A+1] > 0:
        //     R[A]     = R[A] + R[A+2]    (next loop var)
        //     R[A+1]   = R[A+1] - 1       (decrement count)
        //     R[A+3]   = R[A]              (visible loop var copy)
        //     // continue → back-edge (body_loop) or return head_pc
        //   else:
        //     // exit → side-exit at forloop.pc + 1
        //
        // ForLoop is only set at depth=0 (ForLoop@d>0 closes via
        // InlineAbort), so `regs_full[a]` directly addresses the
        // caller window — no offset.
        let rop = &record.ops[for_loop_idx];
        let a = rop.inst.a() as usize;
        match rop.inst.op() {
            Op::ForLoop => {
                let count = bcx.use_var(regs_full[a + 1]);
                let zero = bcx.ins().iconst(types::I64, 0);
                // the loop count is unsigned (PUC `lua_Unsigned`)
                let cond = bcx.ins().icmp(IntCC::NotEqual, count, zero);

                let continue_blk = bcx.create_block();
                let exit_blk = bcx.create_block();
                bcx.ins().brif(cond, continue_blk, &[], exit_blk, &[]);

                // exit branch: side-exit at forloop.pc + 1.
                bcx.switch_to_block(exit_blk);
                bcx.seal_block(exit_blk);
                emit_store_back_and_return_pc(
                    &mut bcx,
                    caller_regs,
                    &stored,
                    reg_state,
                    rop.pc + 1,
                    flush_ctx.as_ref(),
                    0i64,
                    trace_fn_sig_ref,
                    encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
                );

                // continue branch: do the increment + decrement + back-edge.
                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
                let cur = bcx.use_var(regs_full[a]);
                let step = bcx.use_var(regs_full[a + 2]);
                let next = bcx.ins().iadd(cur, step);
                bcx.def_var(regs_full[a], next);
                let one = bcx.ins().iconst(types::I64, 1);
                let count_new = bcx.ins().isub(count, one);
                bcx.def_var(regs_full[a + 1], count_new);
                bcx.def_var(regs_full[a + 3], next);
                // ForLoop's continue branch jumps to the loop's
                // BODY START (= (rop.pc + 1) - bx per OP_FORLOOP's
                // backward jump encoding), not record.head_pc.
                // For trace shapes whose head_pc == body_start
                // (the usual back-edge trace), they're equal.
                // For side traces whose head_pc lands on the
                // ForLoop op itself (head_pc=rop.pc) instead of
                // the back-edge target — e.g. an outer ForLoop
                // that got recorded as a side trace from an inner
                // loop exit — returning record.head_pc would
                // re-enter the ForLoop op and double-advance the
                // counter. A trace headed at an inner loop (a
                // `while` inside the for body) that closes at the
                // outer ForLoop must not loop back to its own head
                // either: that skips the body code before the inner
                // loop. Compute the body start explicitly.
                let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
                let mut tail_kinds = current_kinds[..max_stack].to_vec();
                for k in [a, a + 1, a + 3] {
                    tail_kinds[k] = RegKind::Int;
                }
                if do_internal_loop
                    && body_pc == record.head_pc
                    && loop_kinds_match(&tail_kinds, &head_kinds)
                {
                    sync_reg_state(&mut bcx, &regs_full, &mut stored, reg_state);
                    bcx.ins().jump(body_loop, &[]);
                } else {
                    emit_store_back_and_return_pc(
                        &mut bcx,
                        caller_regs,
                        &stored,
                        reg_state,
                        body_pc,
                        flush_ctx.as_ref(),
                        0i64,
                        trace_fn_sig_ref,
                        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
                    );
                }
            }
            Op::TForLoop => {
                // generic-for back-edge:
                //
                //   tag = tforcall_tag_var  // from TForCall's
                //                           //     batched helper
                //                           //     return value
                //   if tag == NIL:  side-exit at tforloop.pc + 1
                //   elif the key's (and value's) tag is the one the
                //        body was compiled for: R[A+2]=R[A+4] +
                //        back-edge
                //   else: deopt (the interpreter runs the TForLoop)
                //
                // The Nil branch reuses the existing dispatcher
                // restore path; push a per_exit_kinds snapshot with
                // [A+4] = RegKind::Nil so the Nil side-exit repacks
                // correctly (entry's tag for A+4 was Int, so
                // dispatcher without override would restore as Int
                // — wrong for Nil).
                let tag = bcx.use_var(tforcall_tag_var);
                // The body was lowered for the head's entry tags; the
                // back-edge runs it again only with a key (and, when the
                // loop has one, a value) of those tags. A pairs loop
                // over string keys meeting an integer key (or the
                // reverse) stored the new key under the old tag.
                let nvars = match record.ops[for_loop_idx - 1].inst.op() {
                    Op::TForCall => record.ops[for_loop_idx - 1].inst.c() as usize,
                    _ => return None,
                };
                let key_tag = *record.entry_tags.get(a + 4)?;
                let val_tag = if nvars >= 2 {
                    Some(*record.entry_tags.get(a + 5)?)
                } else {
                    None
                };

                let nil_const = bcx
                    .ins()
                    .iconst(types::I64, luna_core::runtime::value::raw::NIL as i64);
                let is_nil = bcx.ins().icmp(IntCC::Equal, tag, nil_const);
                let nil_exit_blk = bcx.create_block();
                let not_nil_blk = bcx.create_block();
                bcx.ins().brif(is_nil, nil_exit_blk, &[], not_nil_blk, &[]);

                // Nil-exit branch: snapshot per_exit_kinds with [A+4]
                // = Nil, then store back + return tforloop.pc + 1.
                bcx.switch_to_block(nil_exit_blk);
                bcx.seal_block(nil_exit_blk);
                // Every loop variable restores as nil: the key is nil, and
                // the value slots hold what the iterator's last call left
                // (nil in the helper path), which the loop no longer reads.
                let mut nil_snapshot: Vec<RegKind> = current_kinds[..max_stack].to_vec();
                for k in (a + 4)..(a + 4 + nvars).min(nil_snapshot.len()) {
                    nil_snapshot[k] = RegKind::Nil;
                }
                let tag_side_box_2: Box<TCellPtr> = Box::new(TCellPtr::null());
                let _tag_side_cell_addr_2 = (&*tag_side_box_2) as *const TCellPtr as i64;
                let tag_side_local_2 = per_exit_kinds.len() as u32;
                per_exit_kinds.push((rop.pc + 1, nil_snapshot, tag_side_box_2));
                emit_tagged_exit(
                    &mut bcx,
                    &mut module,
                    suppress_admit_id,
                    caller_regs,
                    &stored,
                    reg_state,
                    rop.pc + 1,
                    record.head_pc,
                    tag_side_local_2,
                    flush_ctx.as_ref(),
                    trace_fn_sig_ref,
                );

                bcx.switch_to_block(not_nil_blk);
                bcx.seal_block(not_nil_blk);
                let mut same_kinds = bcx.ins().icmp_imm_u(IntCC::Equal, tag, i64::from(key_tag));
                if let Some(val_tag) = val_tag {
                    let v = bcx.use_var(tforcall_val_tag_var);
                    let same_val = bcx.ins().icmp_imm_u(IntCC::Equal, v, i64::from(val_tag));
                    same_kinds = bcx.ins().band(same_kinds, same_val);
                }
                let continue_blk = bcx.create_block();
                let deopt_blk = bcx.create_block();
                bcx.ins()
                    .brif(same_kinds, continue_blk, &[], deopt_blk, &[]);

                // Deopt: the next key or value has another kind than the
                // body was compiled for. Store back + return TForLoop.pc
                // so the interp re-executes the back-edge.
                bcx.switch_to_block(deopt_blk);
                bcx.seal_block(deopt_blk);
                // The helper already wrote the loop variables to the stack
                // with their tags, which are not the ones the registers
                // were compiled for; the dispatcher must leave them there.
                emit_store_back_and_return(
                    &mut bcx,
                    caller_regs,
                    &stored,
                    reg_state,
                    (luna_core::jit::trace_types::EXIT_KEEP_TFOR_VARS | u64::from(rop.pc)) as i64,
                    flush_ctx.as_ref(),
                    0i64,
                    trace_fn_sig_ref,
                    encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
                );

                // Continue: R[A+2] = R[A+4] (ctrl writeback) +
                // back-edge / store_back+head_pc.
                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
                let ctrl = bcx.use_var(regs_full[a + 4]);
                bcx.def_var(regs_full[a + 2], ctrl);
                // as for ForLoop: continue at the loop body, which is
                // the trace head only when the trace was recorded from it
                let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
                // the loop variables passed the tag check above, and the
                // control variable is a copy of the key
                let mut tail_kinds = current_kinds[..max_stack].to_vec();
                let vars = (a + 4)..(a + 4 + nvars.min(2)).min(max_stack);
                tail_kinds[vars.clone()].copy_from_slice(&head_kinds[vars]);
                tail_kinds[a + 2] = head_kinds[a + 4];
                if do_internal_loop
                    && body_pc == record.head_pc
                    && loop_kinds_match(&tail_kinds, &head_kinds)
                {
                    sync_reg_state(&mut bcx, &regs_full, &mut stored, reg_state);
                    bcx.ins().jump(body_loop, &[]);
                } else {
                    emit_store_back_and_return_pc(
                        &mut bcx,
                        caller_regs,
                        &stored,
                        reg_state,
                        body_pc,
                        flush_ctx.as_ref(),
                        0i64,
                        trace_fn_sig_ref,
                        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
                    );
                }
            }
            _ => unreachable!("for_loop_idx_opt only set for Op::ForLoop / Op::TForLoop"),
        }
    } else if do_internal_loop && loop_kinds_match(&current_kinds[..max_stack], &head_kinds) {
        sync_reg_state(&mut bcx, &regs_full, &mut stored, reg_state);
        bcx.ins().jump(body_loop, &[]);
    } else {
        emit_store_back_and_return_pc(
            &mut bcx,
            caller_regs,
            &stored,
            reg_state,
            record.head_pc,
            flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    }
    // Seal the loop head now that both predecessors are emitted
    // (entry → body_loop in the prelude; tail → body_loop from
    // whichever block we ended up in for the clean-close case
    // when internal loop is on).
    bcx.seal_block(body_loop);

    bcx.finalize(module.target_config());
    drop_unused_block_params(&mut ctx.func);
    // `LUNA_TRACE_IR_DUMP=1` dumps the cranelift IR of every
    // compiled trace fn to stderr. Categorization + density-reduction
    // tool for layer-6 attribution (per-call IR op count is the gap).
    if std::env::var("LUNA_TRACE_IR_DUMP")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        eprintln!(
            "=== TRACE IR DUMP head_pc={} n_recorded_ops={} ===\n{}\n=== END ===",
            record.head_pc,
            record.ops.len(),
            ctx.func.display()
        );
    }
    // module finalization is the JIT-specific
    // wrapper's job (see [`try_compile_trace_with_options`]). The
    // generic body emits the function definition and stops at
    // `clear_context`; the JIT wrapper calls `finalize_definitions`
    // + `get_finalized_function`, patches `compiled.entry` with the
    // real fn pointer, and parks the module on the Vm's
    // `storage.trace_handles` Vec.
    // The AOT pipeline (luna-aot) calls `ObjectModule::finish` /
    // `ObjectProduct::emit` to produce a `.o` file instead, and
    // resolves the trace symbol at static link time.

    // Op::ForLoop at the tail writes R[A] (next loop var), R[A+1]
    // (decremented count), and R[A+3] (visible loop var copy) —
    // all Int per the 5.4+ count form. Op::TForLoop writes R[A+2]
    // = R[A+4] on continue (TForLoop tail emit; R[A+4] = Int gated
    // by the tag check).
    if let Some(for_loop_idx) = for_loop_idx_opt {
        let rop = &record.ops[for_loop_idx];
        let a = rop.inst.a() as usize;
        match rop.inst.op() {
            Op::ForLoop => {
                current_kinds[a] = RegKind::Int;
                current_kinds[a + 1] = RegKind::Int;
                current_kinds[a + 3] = RegKind::Int;
            }
            Op::TForLoop => {
                current_kinds[a + 2] = RegKind::Int;
            }
            _ => {}
        }
    }
    // Derive exit_tags from the kind tracker's final state. Slots
    // the trace never touched stay `Untouched` (dispatcher restores
    // the entry tag); slots the trace wrote take the writer's
    // determined kind. `current_kinds` propagates source kinds at
    // the Move op so the dispatcher doesn't need a deferred
    // entry-tag lookup.
    // dispatch heuristic: a `Op::Call`-truncated
    // trace whose body is too short to amortise the dispatcher's
    // marshal-in + transmute + restore overhead is a net loss vs the
    // interpreter (measured at ~1.8× slower on fib_28's ~7-op body).
    // Keep such traces cached (compile cost is paid) but pin
    // dispatchable=false unless the per-dispatch body is large
    // enough to win. `MIN_DISPATCHABLE_TRUNC_BODY_BASE` is tuned to fib's
    // 7-op body being just below the gate at depth=0.
    //
    // scale the gate down as `max_depth_used` grows:
    // each extra inline level amortises ~2 ops worth of marshal
    // overhead per dispatch (one dispatch processes the full
    // chain of depth+1 frames). Saturating-sub so deep traces
    // never miss-fire on the length gate.
    const MIN_DISPATCHABLE_TRUNC_BODY_BASE: usize = 20;
    // floor at 40 ops/dispatch (the empirical
    // dispatcher-overhead amortisation line: ~80ns per dispatch /
    // ~2ns per body op). The adaptive `BASE - depth*2`
    // formula alone could drop the gate to 0 at MAX_INLINE_DEPTH=16,
    // letting tiny-body inline traces dispatch and pay overhead
    // they can't amortise (binary_trees_d4 runs 0.73× without the
    // floor). The floor doesn't affect fib_28 (~112 ops body —
    // well above the floor) but bails the binary_trees
    // pathological case.
    const MIN_DISPATCHABLE_TRUNC_BODY_FLOOR: usize = 40;
    let max_depth_used = record
        .ops
        .iter()
        .map(|r| r.inline_depth as usize)
        .max()
        .unwrap_or(0);
    let adaptive = MIN_DISPATCHABLE_TRUNC_BODY_BASE.saturating_sub(max_depth_used * 2);
    let min_dispatchable_trunc_body = adaptive.max(MIN_DISPATCHABLE_TRUNC_BODY_FLOOR);
    // inline traces (per_exit_metas non-empty)
    // skip the length-gate. Each dispatch tears through multiple
    // inlined frames so body-length isn't a useful proxy for the
    // dispatcher's marshal overhead; the gate would dump fib's
    // ~8-op-by-the-time-MAX_DEPTH-hits prefix even though one
    // dispatch processes 4 recursion levels.
    //
    // sunk-alloc traces also skip the length-gate.
    // Skipping even a single `Heap::new_table()` per dispatch
    // dwarfs the marshal-in/out overhead on a 7-op body, so the
    // gate's conservative default is a net loss here.
    //
    // Closure-creating traces do NOT skip the
    // length-gate. Unlike sunk emit which avoids `Heap::new_table()`,
    // the Op::Closure helper still calls `Heap::new_closure_inline`
    // — emit replaces only the interp's match-arm dispatch +
    // frame plumbing for the 2-op `Closure + Return1` shape, which
    // is less than trace dispatch's marshal+enter overhead. Per-iter
    // dispatch of a tiny closure-constructor body is a net loss
    // (probe: `closure_no_upval_for_500k` mac measured 0.53× when
    // the gate was skipped). Closure traces only earn dispatch when
    // body length passes the gate organically.
    // InlineAbort traces close without materialising frames. The
    // interp can't resume at the inline-abort PC without the
    // matching CallFrames, so gate dispatch off.
    // Recorded before the length gate: the first reason is the one
    // kept, and side-trace wiring tells a trace that is unsafe to run
    // from one that is only too short to dispatch by it.
    if inline_abort_idx_opt.is_some() {
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("InlineAbort-gate"));
    }
    if (call_idx_opt.is_some() || return_idx_opt.is_some())
        && effective_end < min_dispatchable_trunc_body
        && per_exit_inline_vec.is_empty()
        && sunk_alloc_seen == 0
    {
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("length-gate"));
    }

    // clean-tail `exit_tags` cover the caller window
    // only ([0..max_stack)). Per-side-exit `per_exit_tags` for inline
    // cmp sites carry the full `window_size` snapshot
    // because the dispatcher must restore EVERY pushed frame's
    // register window, not just the caller's.
    let mut exit_tags_vec = kinds_to_exit_tags(&current_kinds[..max_stack]);
    // for every sunk site at depth=0 (depth>0 is rejected
    // in pre-emit), force the slot's exit tag to `Untouched` so the
    // dispatcher carries the entry tag in the restore. Without this
    // override the slot's `current_kinds` could read as Table (from
    // some other path) or Unset, and the dispatcher would try to
    // unpack `reg_state[a]` (which we never wrote for sunk sites)
    // as a `Value::Table` of NULL bits → SIGSEGV.
    for site in &escape.sites {
        if site.state == EscapeState::Sinkable && site.inline_depth == 0 {
            let idx = site.a as usize;
            if idx < exit_tags_vec.len() {
                exit_tags_vec[idx] = ExitTag::Untouched;
            }
        }
    }
    let global_tag_res_kind = classify_exit_tags(&exit_tags_vec);
    let exit_tags: TArc<[ExitTag]> = exit_tags_vec.into();
    // split per_exit_kinds's 3-tuple into the
    // 2-tuple `per_exit_tags` for the dispatcher AND the parallel
    // `tags_side_trace_ptrs` Box slice the close handler writes to.
    // The Box transports the cell's heap address (baked into the
    // IR's `iconst` at each callsite) through this move without
    // moving the cell itself.
    let mut tags_side_boxes: Vec<Box<TCellPtr>> = Vec::with_capacity(per_exit_kinds.len());
    let per_exit_tags: TArc<[(u32, TArc<[ExitTag]>)]> = per_exit_kinds
        .into_iter()
        .map(|(pc, kinds, side_box)| {
            // The cmp emit site pushed the right slice
            // length (caller-window for depth=0, full window for
            // depth>0). Hand it through verbatim — the dispatcher
            // iterates `exit_tags_for_pc.len()` and walks both
            // shapes uniformly.
            let tags: TArc<[ExitTag]> = kinds_to_exit_tags(&kinds).into();
            tags_side_boxes.push(side_box);
            (pc, tags)
        })
        .collect::<Vec<_>>()
        .into();
    let tags_side_trace_ptrs: TArc<[Box<TCellPtr>]> = tags_side_boxes.into();
    let per_exit_inline: TArc<[InlineSideExit]> = per_exit_inline_vec
        .into_iter()
        .map(
            |(cont_pc, head_resume_pc, kinds, chain, side_trace_ptr)| InlineSideExit {
                cont_pc,
                head_resume_pc,
                exit_tags: kinds_to_exit_tags(&kinds).into(),
                chain,
                side_trace_ptr,
            },
        )
        .collect::<Vec<_>>()
        .into();

    checkpoint("post:emit-pass-done");
    // pre-compute exit_hit_counts before the struct
    // init so per_exit_tags's len is still accessible.
    let exit_hit_counts: TArc<[TCellU32]> = {
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let v: Vec<TCellU32> = (0..total).map(|_| TCellU32::new(0)).collect();
        v.into()
    };
    // parallel per-exit raw fn-ptr slots, all null
    // until a child side trace compiles for the slot. Same length
    // as exit_hit_counts.
    let exit_side_trace_ptrs: TArc<[TCellPtr]> = {
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let v: Vec<TCellPtr> = (0..total).map(|_| TCellPtr::null()).collect();
        v.into()
    };
    let compiled = CompiledTrace {
        head_pc: record.head_pc,
        // caller (JIT wrapper or AOT pipeline)
        // patches `entry` after finalize. See [`placeholder_trace_fn`].
        entry: placeholder_trace_fn,
        n_ops: record.ops.len() as u32,
        dispatchable,
        // real window_size ≥ max_stack; the
        // dispatcher reads this to size its reg_state buffer.
        window_size,
        exit_tags,
        global_tag_res_kind,
        is_inline_abort_close: inline_abort_idx_opt.is_some(),
        dispatch_off_reason: if dispatchable {
            None
        } else {
            dispatch_off_reason
        },
        entry_tags: record
            .entry_tags
            .iter()
            .enumerate()
            .map(|(i, &t)| match head_live.get(i) {
                Some(false) => ENTRY_TAG_ANY,
                _ => t,
            })
            .collect::<Vec<u8>>()
            .into(),
        per_exit_tags,
        // populated by the cmp@d>0 emit sites
        // above; the IR encodes `(site_idx + 1)` in the upper 32
        // bits of its return value so the dispatcher can pull the
        // right entry. Holding the inner Rc<[FrameMaterializeInfo]>
        // alive keeps each chain's address stable across dispatches
        // (cranelift IR has the raw pointer baked in via iconst).
        exit_hit_counts,
        exit_side_trace_ptrs,
        // per-TAG-entry side-trace cells (parallel
        // to per_exit_tags) + the GLOBAL singleton cell. Both
        // collected from Boxes allocated AT each emit callsite so
        // the IR has baked the right heap address.
        tags_side_trace_ptrs,
        global_side_trace_ptr: global_side_trace_box,
        // empty at compile; close handler fills
        // it as child side traces compile for this trace's hot
        // exits.
        side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
        has_any_side_wired: TCellBool::new(false),
        per_exit_inline,
        // diagnostic only; counts Sinkable sites from the
        // pre-emit sweep. Vm sums these into
        // `trace_sinkable_seen_count`.
        sinkable_sites_seen: escape.sinkable_count(),
        accum_bufferable_seen: escape
            .accum_sites
            .iter()
            .filter(|s| s.state == BufferState::Bufferable)
            .count() as u32,
        // count of sites that actually took the sunk-emit
        // path in this trace's body (NewTable replaced by virt slot
        // Variables, no heap alloc helper called). Vm bumps
        // `trace_sunk_alloc_count` by this on compile success.
        sunk_alloc_seen,
        // count of materialise emit sites for sunk slot
        // recovery at cmp side-exits.
        materialize_emit_count,
        // count of Op::Closure ops the trace lowered.
        closure_seen,
        // compute body_writes for the smart side-trace
        // gate. Uses op_offsets (already computed above) to apply
        // inline-depth offsets per op.
        body_writes: compute_body_writes(record, &op_offsets).into(),
        // populated by the `downrec_idx_opt` arm
        // above into `downrec_link_for_compiled`. When the arm
        // emitted a stitch sentinel + caller-pc guard, this carries
        // `Some((0, head_pc))`; otherwise `None`. `Some(_)` alone
        // doesn't make the trace dispatchable — that takes a
        // multi-way candidate count >= 2 (see
        // `downrec_multi_way_count` below).
        downrec_link: downrec_link_for_compiled,
        // multi-way guard candidate count baked
        // into the IR's CMP-chain. `0` for non-DownRec closes;
        // `1` for single-CMP-fallback DownRec; `>= 2` for the
        // lifted `dispatchable = true` path.
        downrec_multi_way_count: downrec_multi_way_count_for_compiled,
    };
    // decided only now: the dispatch gates above run after the emit pass
    if always_codegen || trace_is_enterable(record, &compiled) {
        // `LUNA_TRACE_ASM_DUMP=1` requests cranelift to
        // emit the post-regalloc machine-code disassembly (vcode) and dumps
        // it to stderr after `define_function`. Used for the cargo-asm
        // decomposition of the table-field IC under env-OFF vs env-ON.
        let want_asm_dump = std::env::var("LUNA_TRACE_ASM_DUMP")
            .map(|v| v == "1")
            .unwrap_or(false);
        if want_asm_dump {
            ctx.set_disasm(true);
        }
        module.define_function(fn_id, &mut ctx).ok()?;
        if want_asm_dump
            && let Some(cc) = ctx.compiled_code()
            && let Some(vcode) = cc.vcode.as_ref()
        {
            eprintln!(
                "=== TRACE ASM DUMP head_pc={} n_recorded_ops={} ===\n{}\n=== END ===",
                record.head_pc,
                record.ops.len(),
                vcode
            );
        }
        module.clear_context(&mut ctx);
    }
    Some((fn_id, compiled))
}
