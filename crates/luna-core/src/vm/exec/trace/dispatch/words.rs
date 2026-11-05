//! The words past a trace's window a dispatch shares with the code (see
//! `crate::__private::trace_count_slot`).

use crate::__private::{TRACE_FRAME_TAIL, trace_count_slot, trace_passed_slot};
use crate::jit::trace::CompiledTrace;

/// Readies the Vm's buffer `reg_state` for a call of a trace whose window is
/// `window` registers wide: at least the window and the words past it,
/// grown only when a trace wider than any before runs; the window's
/// registers past the frame's `max_stack`, which marshal-in fills, are
/// zero. The words past the window are left as they are: the ones the call
/// reads are set by [`count_in`] and the down-recursion entry.
#[inline(always)]
pub(super) fn frame_in(reg_state: &mut Vec<i64>, window: usize, max_stack: usize) {
    if reg_state.len() < window + TRACE_FRAME_TAIL {
        grow(reg_state, window + TRACE_FRAME_TAIL);
    }
    reg_state[max_stack.min(window)..window].fill(0);
}

#[cold]
#[inline(never)]
fn grow(reg_state: &mut Vec<i64>, len: usize) {
    reg_state.resize(len, 0);
}

/// A trace that counts in this call's word (`CompiledTrace::call_count`)
/// gets the trace's count in both halves of it, the low one for the code to
/// count on, the high one kept as the count the call started from, and its
/// mark of a pass into the loop cleared. Any other trace reads neither
/// word: its code counts in the trace's count cell.
#[inline(always)]
pub(in crate::vm::exec) fn count_in(ct: &CompiledTrace, regs: &mut [i64], window: usize) {
    if ct.call_count
        && let Some(t) = &ct.tier_up
    {
        let c = u64::from(t.count.get());
        let w = &mut regs[trace_count_slot(window)..=trace_passed_slot(window)];
        w[0] = (c << 32 | c) as i64;
        w[1] = 0;
    }
}

/// After the call: the low half of the call's count word back into the
/// trace's count (see [`count_in`]).
#[inline(always)]
pub(in crate::vm::exec) fn count_out(ct: &CompiledTrace, regs: &[i64], window: usize) {
    if ct.call_count
        && let Some(t) = &ct.tier_up
    {
        t.count.set(regs[trace_count_slot(window)] as u32);
    }
}

/// Whether the call of a trace that counts in this call's word, run on `regs` with
/// window `window`, went round a back edge: its mark of a pass into the
/// loop is set, or its count moved off the one it started from (see
/// [`count_in`]). Only meaningful for such a trace.
pub(in crate::vm::exec) fn call_passed(regs: &[i64], window: usize) -> bool {
    let w = regs[trace_count_slot(window)] as u64;
    regs[trace_passed_slot(window)] != 0 || w as u32 != (w >> 32) as u32
}
