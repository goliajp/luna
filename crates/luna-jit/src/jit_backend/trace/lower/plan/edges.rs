//! Ops the whitelist does not name that the trace still runs: the inline
//! path's calls and returns, and the loop ops inside a linked side trace.

use super::*;

/// An op of an inlined frame's entry or exit: `Some` with the verdict, or
/// `None` for any other op.
pub(super) fn validate_frame_op(rop: &RecordedOp, max_stack: usize) -> Option<Option<()>> {
    let op = rop.inst.op();
    // self-recursive Op::Call inside the inline
    // path emits no IR (the next op shifts to the callee window
    // via op_offsets). It's not in `is_whitelisted_op`, so
    // accept it explicitly when depth>0 OR when the next op is
    // at depth+1 (the recorder's self-recursive marker).
    if matches!(op, Op::Call) {
        // The terminator pass already let this Op::Call past as
        // a self-recursive call. Skip the whitelist check.
        return Some(Some(()));
    }
    // Op::Return0 / Op::Return1 at depth>0 are
    // the inline path's unwind ops. They're not in the
    // whitelist (it only covers depth=0 ops with no return
    // semantics); admit them when depth>0.
    if rop.inline_depth > 0 && matches!(op, Op::Return0 | Op::Return1 | Op::Return) {
        // the values returned sit in the frame (`inline_calls` fixed the
        // count of a `Return`)
        let n = match op {
            Op::Return0 => 0,
            Op::Return1 => 1,
            _ if rop.inst.b() > 0 => rop.inst.b() - 1,
            _ => 1,
        };
        if (rop.inst.a() + n) as usize > max_stack {
            checkpoint("bail:cmp-dirs-Return1-a-oob");
            return Some(None);
        }
        return Some(Some(()));
    }
    // a vararg expansion in a vararg function the trace inlined: the extra
    // arguments sit below the frame's registers, as many as the call
    // passed (the head frame's are on the stack, which the trace does not
    // read)
    if matches!(op, Op::Vararg) {
        let fits = rop.inline_depth > 0 && rop.proto.is_vararg && {
            let n = rop.inst.c().saturating_sub(1);
            (rop.inst.a() + n) as usize <= max_stack
        };
        if !fits {
            checkpoint("bail:vararg-outside-inlined-frame");
            return Some(None);
        }
        return Some(Some(()));
    }
    None
}

/// A numeric `for` loop's `ForLoop` or `ForPrep` inside a side trace
/// linked to the compiled trace it reached (elsewhere the loop edge ends
/// the trace, and the recorder starts after the `ForPrep`).
pub(super) fn validate_mid_loop_op(
    record: &TraceRecord,
    rop: &RecordedOp,
    a: usize,
    max_stack: usize,
) -> Option<()> {
    assert!(
        !rop.inst.op().is_for_loop() || (rop.inline_depth == 0 && record.link_pc.is_some()),
        "Op::ForLoop only appears at effective_end (loop-end guarded above)"
    );
    // 5.5 keeps the loop variable in the third state register
    let last = match rop.inst.op() {
        Op::ForLoop55 | Op::ForLoop55F | Op::ForPrep55 => a + 2,
        _ => a + 3,
    };
    if rop.inline_depth != 0 || record.link_pc.is_none() || last >= max_stack {
        checkpoint("bail:mid-for-loop");
        return None;
    }
    Some(())
}
