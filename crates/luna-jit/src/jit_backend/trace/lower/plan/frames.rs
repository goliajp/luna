use super::*;

/// The record's frame shape: per-op register-window offsets and the
/// window size.
pub(super) fn plan_frames(
    record: &TraceRecord,
    head_proto: Gc<Proto>,
    frame_w: usize,
    calls: &[Option<InlineCall>],
) -> Option<(Vec<u32>, u32)> {
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
    let (op_offsets, _) = compute_op_offsets_with(record, calls);
    let mut window_size: u32 = op_offsets
        .iter()
        .map(|&off| off + frame_w as u32)
        .max()
        .unwrap_or(frame_w as u32);
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
            let needed = bump_off + frame_w as u32;
            if needed > window_size {
                window_size = needed;
            }
        }
    }
    Some((op_offsets, window_size))
}

pub(super) fn side_trace_gate(record: &TraceRecord, op_offsets: &[u32]) -> Option<()> {
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
    if let Some((parent_proto, parent_head_pc, parent_exit)) = record.side_trace_parent {
        // Check 1: any back-edge op? (ForLoop / TForLoop / Jmp -bx)
        let has_back_edge = record.ops.iter().any(|op| match op.inst.op() {
            o if o.is_jump() => op.inst.jump_offset() < 0,
            o => o.is_for_loop() || o.is_tfor_loop(),
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
                        | Op::TForCall53
                        | Op::TForCall55
                        | Op::SetTable
                        | Op::SetI
                        | Op::SetField
                        | Op::SetUpval
                        | Op::SetTabUp
                        | Op::Closure
                        | Op::Close
                        | Op::JmpClose
                        | Op::JmpCloseBack
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
                // the child's register `r` is the parent's `off + r` (the
                // frame the parent's exit resumes in)
                let parent_writes_opt = {
                    let traces = parent_proto.traces.borrow();
                    traces
                        .iter()
                        .find(|t| t.head_pc == parent_head_pc)
                        .map(|pct| {
                            let off = pct.exit_frame_offset(parent_exit) as u32;
                            pct.body_writes
                                .iter()
                                .filter_map(|&w| w.checked_sub(off))
                                .collect::<Vec<u32>>()
                        })
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
