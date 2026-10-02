use super::*;

mod validate;
use validate::*;

// detect the FIRST Bufferable AccumSite.
// Buffered emit handles a single site. The 4 idiom op indices
// are pre1 = op_idx-2, pre2 = op_idx-1, concat = op_idx,
// post = op_idx+1.
#[derive(Clone, Copy, Debug)]
pub(super) struct BufferedAccum {
    pub(super) accum_slot: u32,
    pub(super) piece_slot: u32,
    pub(super) pre1_idx: usize,
    pub(super) pre2_idx: usize,
    pub(super) concat_idx: usize,
    pub(super) post_idx: usize,
}

/// What the pre-emit passes found out about a trace: its register
/// windows, where it ends, which ops are folded or consumed, and the
/// shape of its tail. Read-only once built.
pub(super) struct Plan<'r> {
    pub(super) record: &'r TraceRecord,
    pub(super) head_proto: Gc<Proto>,
    pub(super) max_stack: usize,
    pub(super) vconsts: Vec<Option<VConst>>,
    pub(super) opts: CompileOptions,
    pub(super) float_only: bool,
    pub(super) op_offsets: Vec<u32>,
    pub(super) enclosing_call_a: Vec<Option<u8>>,
    pub(super) window_size: u32,
    pub(super) window_size_us: usize,
    pub(super) folded_ops: Vec<bool>,
    pub(super) math_folds: Vec<TraceMathFold>,
    pub(super) end_idx_opt: Option<(usize, TraceEnd)>,
    pub(super) effective_end: usize,
    pub(super) call_idx_opt: Option<usize>,
    pub(super) for_loop_idx_opt: Option<usize>,
    pub(super) inline_abort_idx_opt: Option<usize>,
    pub(super) return_idx_opt: Option<usize>,
    pub(super) self_link_idx_opt: Option<(usize, SelfRecKind)>,
    pub(super) downrec_idx_opt: Option<(usize, u32, usize, u8)>,
    pub(super) do_internal_loop: bool,
    pub(super) head_live: Vec<bool>,
    pub(super) active_accum: Option<BufferedAccum>,
    pub(super) consumed_by_cmp: Vec<bool>,
    pub(super) cmp_dirs: Vec<Option<CmpDir>>,
}

/// The constant held by the virtual register of op `i`, if it has one.
pub(super) fn vconst_at(vconsts: &[Option<VConst>], i: usize) -> Option<VConst> {
    vconsts.get(i).copied().flatten()
}

/// The pre-emit passes. `None` when the trace cannot be compiled; the
/// escape analysis comes back separately because emit demotes sites.
pub(super) fn plan_trace<'r>(
    record: &'r TraceRecord,
    vconsts: Vec<Option<VConst>>,
    head_proto: Gc<Proto>,
    max_stack: usize,
    opts: CompileOptions,
    float_only: bool,
) -> Option<(Plan<'r>, EscapeAnalysis)> {
    let n = record.ops.len();

    let (op_offsets, enclosing_call_a, window_size) = plan_frames(record, head_proto, max_stack)?;
    let window_size_us = window_size as usize;

    side_trace_gate(record, &op_offsets)?;
    validate_inline_calls(record, head_proto)?;
    let (folded_ops, math_folds) = scan_math_folds(record, n, head_proto, opts);
    let end_idx_opt = find_trace_end(record, &folded_ops, head_proto, n)?;
    let effective_end = end_idx_opt.map(|(i, _)| i).unwrap_or(n);
    // escape analysis over the recorded body +
    // terminator. The pre-emit pass below demotes any Sinkable
    // site that doesn't meet the sunk-emit criteria back to
    // Escaped, so emit only honours sites we actually allocate
    // virt-slot Variables for.
    checkpoint("post:end-idx-found");
    let escape = escape_analyze(
        record,
        effective_end,
        end_idx_opt.map(|(_, k)| k),
        head_proto,
    );
    checkpoint("post:escape-analyze");
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

    let (consumed_by_cmp, cmp_dirs) = validate_ops(
        record,
        &vconsts,
        head_proto,
        max_stack,
        effective_end,
        &folded_ops,
    )?;
    validate_trace_ends(
        record,
        head_proto,
        max_stack,
        opts,
        effective_end,
        &consumed_by_cmp,
        call_idx_opt,
        return_idx_opt,
        for_loop_idx_opt,
    )?;
    Some((
        Plan {
            record,
            head_proto,
            max_stack,
            vconsts,
            opts,
            float_only,
            op_offsets,
            enclosing_call_a,
            window_size,
            window_size_us,
            folded_ops,
            math_folds,
            end_idx_opt,
            effective_end,
            call_idx_opt,
            for_loop_idx_opt,
            inline_abort_idx_opt,
            return_idx_opt,
            self_link_idx_opt,
            downrec_idx_opt,
            do_internal_loop,
            head_live,
            active_accum,
            consumed_by_cmp,
            cmp_dirs,
        },
        escape,
    ))
}

/// The record's frame shape: per-op register-window offsets, the
/// enclosing call's A per op, and the window size.
fn plan_frames(
    record: &TraceRecord,
    head_proto: Gc<Proto>,
    max_stack: usize,
) -> Option<(Vec<u32>, Vec<Option<u8>>, u32)> {
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
    Some((op_offsets, enclosing_call_a, window_size))
}

fn side_trace_gate(record: &TraceRecord, op_offsets: &[u32]) -> Option<()> {
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
            let child_live_in = compute_live_in_slots(record, op_offsets);
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
    Some(())
}

fn validate_inline_calls(record: &TraceRecord, head_proto: Gc<Proto>) -> Option<()> {
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
    Some(())
}

fn scan_math_folds(
    record: &TraceRecord,
    n: usize,
    head_proto: Gc<Proto>,
    opts: CompileOptions,
) -> (Vec<bool>, Vec<TraceMathFold>) {
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
    (folded_ops, math_folds)
}

fn find_trace_end(
    record: &TraceRecord,
    folded_ops: &[bool],
    head_proto: Gc<Proto>,
    n: usize,
) -> Option<Option<(usize, TraceEnd)>> {
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
    let plain_end = plain_trace_end(record, folded_ops, head_proto);
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
    Some(end_idx_opt)
}
