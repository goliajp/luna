//! The pre-emit checks of the ops that end a trace's body.

use super::*;

/// The ops that end the body: stray `Jmp`s, and the truncating call,
/// the depth-0 return or the loop edge at `effective_end`.
#[allow(clippy::too_many_arguments)]
pub(super) fn validate_trace_ends(
    record: &TraceRecord,
    head_proto: Gc<Proto>,
    max_stack: usize,
    effective_end: usize,
    consumed_by_cmp: &[bool],
    call_idx_opt: Option<usize>,
    return_idx_opt: Option<usize>,
    for_loop_idx_opt: Option<usize>,
) -> Option<()> {
    // Jmp validation inside the normal range. A Jmp is OK if it
    // was consumed by a preceding cmp (handled above) or sits at
    // the effective end's last position (the back-edge that closes
    // the loop, or the slot right before an Op::Call truncation —
    // the tail / side-exit emits the control transfer).
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        if rop.inst.op().is_jump()
            && !consumed_by_cmp[i]
            && i + 1 != effective_end
            && !jumps_to_next(rop, &record.ops[i + 1], record.side_trace_parent.is_some())
        {
            checkpoint("bail:body-jmp");
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
        // a numeric loop touches its state registers and the loop
        // variable; a generic one reads the first variable (the key the
        // iterator returned) and may write the control. All must fit in
        // the frame. Which form steps a numeric loop is decided from the
        // kinds at the tail (`emit_for_loop_tail`).
        let lay = rop.inst.op().for_layout()?;
        if a + lay.var() as usize >= max_stack {
            return None;
        }
    }
    Some(())
}

/// A jump the recording followed to `next`: the trace goes on there, and
/// the jump itself needs no code (a closing jump's close is emitted with the
/// body). Forward (the end of an `if` branch skipping the `else`) in any
/// trace; backward (the loop edge of a loop the trace runs into) in a side
/// trace, which runs its ops once from its head to where it ends and never
/// loops back itself.
fn jumps_to_next(jmp: &RecordedOp, next: &RecordedOp, side: bool) -> bool {
    let target = i64::from(jmp.pc) + 1 + i64::from(jmp.inst.jump_offset());
    (jmp.inst.jump_offset() >= 0 || side)
        && next.inline_depth == jmp.inline_depth
        && std::ptr::eq(next.proto.as_ptr(), jmp.proto.as_ptr())
        && i64::from(next.pc) == target
}
