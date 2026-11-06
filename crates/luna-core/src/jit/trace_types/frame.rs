//! The buffer a trace function runs on (`reg_state`): the registers of
//! its window, `0..window`, then three words the dispatcher and the code
//! share for one call:
//!
//! | word         | meaning                                                  |
//! |--------------|----------------------------------------------------------|
//! | `window`     | a down-recursion trace's saved caller pc                 |
//! | `window + 1` | this call's tier-up count ([`trace_count_slot`]): the dispatcher copies the trace's count into both 32-bit halves before the call and the low half back after it; the baseline tier's back edge adds one to the low half, the high half keeps the count the call started from |
//! | `window + 2` | not 0 once this call passed a back edge into the optimizing tier's loop ([`trace_passed_slot`]): the dispatcher clears it, the back edge's hand-over sets it |
//!
//! A call has passed its first back edge when word `window + 2` is set or
//! the two halves of word `window + 1` differ. Only a trace whose exits
//! tell a call before its first back edge from one past it (its plans count
//! `PassCount::PerCall`) counts in these two words; any other counts in
//! its tier-up count cell, and the dispatcher leaves the words alone. Each call has its words: a
//! trace entered again from inside itself (a metamethod running the same
//! loop) counts in its own buffer.

/// The words past the window a trace's buffer holds.
pub(crate) const TRACE_FRAME_TAIL: usize = 3;

/// The word of a buffer whose window is `window` registers wide that holds
/// this call's tier-up count.
pub const fn trace_count_slot(window: usize) -> usize {
    window + 1
}

/// The word that is set once this call went round a back edge into the
/// optimizing tier's loop.
pub const fn trace_passed_slot(window: usize) -> usize {
    window + 2
}
