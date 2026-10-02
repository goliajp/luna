use super::*;

pub(super) fn validate_inline_calls(record: &TraceRecord, head_proto: Gc<Proto>) -> Option<()> {
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

pub(super) fn scan_math_folds(
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

pub(super) fn find_trace_end(
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
