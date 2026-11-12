//! A side-trace recording that closed at its first op, a return out of the
//! frame it began in. Counted by its close cause, not in a counter of its
//! own: a field would grow the Vm and move the offsets the dispatch reads.

/// The close cause of a side-trace recording that closed at its first op,
/// a return out of the frame it began in: its code would only return the
/// pc it starts at, so it is neither compiled nor wired.
pub(super) const SIDE_TRACE_EMPTY: &str = "side-trace-empty";

/// Whether `record`, a side trace's, closed at its first op, a return out
/// of the frame it began in: compiled, it would return the pc it starts at
/// without running anything, and running it at its parent's exit would
/// only cost the exit its plan. Its parent's exit is not recorded again
/// (its count passed the threshold that starts a recording).
pub(super) fn returns_at_start(record: &crate::jit::trace::TraceRecord) -> bool {
    use crate::vm::isa::Op;
    record.side_trace_parent.is_some()
        && matches!(
            record.ops.first(),
            Some(o) if o.inline_depth == 0 && matches!(o.inst.op(), Op::Return0 | Op::Return1)
        )
}
