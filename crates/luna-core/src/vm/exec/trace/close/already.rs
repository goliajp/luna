//! Whether a closed recording would add a trace that is there already, or
//! one that may not be made.

use crate::jit::trace::TraceRecord;
use crate::vm::isa::Op;

/// A trace that is no side trace starts at the recording's head already
/// (one that could not be dispatched as well: the head is not recorded
/// again), or a side trace's parent has one at its exit already or is
/// gone. A side trace may start where other traces do. An entry side
/// trace's recording: as `entry_side_there_already`.
///
/// A side trace starting in a function its parent inlined that reads or
/// writes an upvalue is not made either: the upvalue may be a register of
/// a frame the parent runs, which the parent's code holds without storing
/// it to the stack the upvalue reads when the side trace runs from there.
pub(super) fn there_already(rec: &TraceRecord) -> bool {
    if rec.entry_side {
        return super::super::entry_side::entry_side_there_already(rec);
    }
    match rec.side_trace_parent {
        Some((parent, exit)) => parent.trace().is_none_or(|p| {
            p.has_side_trace(exit) || (p.exit_frame_offset(exit) > 0 && touches_upvalues(rec))
        }),
        None => (rec.head_proto.traces.borrow().iter())
            .any(|t| t.head_pc == rec.head_pc && !t.is_side_trace()),
    }
}

fn touches_upvalues(rec: &TraceRecord) -> bool {
    (rec.ops.iter()).any(|o| matches!(o.inst.op(), Op::GetUpval | Op::SetUpval))
}
