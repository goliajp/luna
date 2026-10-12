//! What the loop head hands the fast loop, and what the fast loop hands
//! back.

use super::*;

/// The running frame, as the loop head found it.
pub(in crate::vm::exec) struct Fast {
    /// the running frame, on top of `frames`
    pub(in crate::vm::exec) fr: *mut Frame,
    pub(in crate::vm::exec) trace_on: bool,
    pub(in crate::vm::exec) entry_depth: usize,
    /// false when some instruction needs the loop head's checks (a trap,
    /// a recording, or more trace heads than `heads` holds)
    pub(in crate::vm::exec) stay: bool,
    /// the pcs where a trace this function could enter starts
    pub(in crate::vm::exec) heads: [u32; crate::runtime::function::TRACE_HEADS_CAP],
}

/// Why the fast loop handed control back.
pub(in crate::vm::exec) enum FastExit {
    /// the frame state may have changed: back to the loop head
    Reload,
    /// an opcode the loop head's own match runs
    Slow(Inst),
}
