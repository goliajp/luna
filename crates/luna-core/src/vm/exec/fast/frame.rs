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

/// Where the loop goes after an arm run out of line (see `arm_fns`).
#[cfg(not(luna_hot))]
pub(super) enum Flow {
    /// the next instruction (`next!`)
    Next,
    /// the next instruction after a jump (`next_jumped!`)
    NextJumped,
    /// the same frame after a slow path that may have moved it (`resume!`)
    Resume,
    /// the same frame after a slow path that moved neither the pc nor the
    /// stack (`resume_same!`)
    ResumeSame,
    /// whatever frame is now on top (`reenter!`)
    Reenter,
    /// the frame `fr` now names, from its pc
    Frames,
    /// after a return (`returned!`)
    Returned(super::call_fast::Returned),
    /// back to the loop head
    Exit(FastExit),
}

/// The loop's locals an arm run out of line reads and writes.
#[cfg(not(luna_hot))]
#[derive(Clone, Copy)]
pub(super) struct St {
    pub(super) fr: *mut Frame,
    pub(super) regs: *mut Value,
    pub(super) npc: u32,
    pub(super) inst: Inst,
    pub(super) code: *const Inst,
    pub(super) kptr: *const Value,
    pub(super) trace_on: bool,
    pub(super) entry_depth: usize,
    pub(super) stay: bool,
    pub(super) heads: [u32; crate::runtime::function::TRACE_HEADS_CAP],
}

/// An arm's function (see `arm_fns`).
#[cfg(not(luna_hot))]
pub(super) type ArmFn = fn(&mut Vm, &mut St) -> Result<Flow, crate::vm::LuaError>;

/// Why the fast loop handed control back.
pub(in crate::vm::exec) enum FastExit {
    /// the frame state may have changed: back to the loop head
    Reload,
    /// an opcode the loop head's own match runs
    Slow(Inst),
}
