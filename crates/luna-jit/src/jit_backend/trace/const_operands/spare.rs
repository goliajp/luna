//! The record copy the constant-operand rewrite works on, in buffers kept
//! from the last one.

use luna_core::__private::TraceRecord;

thread_local! {
    static SPARE_RECORD: std::cell::RefCell<Option<TraceRecord>> =
        const { std::cell::RefCell::new(None) };
}

/// A copy of `src` in the buffers of the record last given back on this
/// thread ([`give_record`]).
pub(super) fn copy_record(src: &TraceRecord) -> TraceRecord {
    let Some(mut out) = SPARE_RECORD.with(|s| s.borrow_mut().take()) else {
        return src.clone();
    };
    // every field by name: one added to the record does not compile here
    // until it is copied
    let TraceRecord {
        head_proto,
        settings,
        head_pc,
        entry_tags,
        ops,
        closed,
        is_call_triggered,
        tfor_iter,
        tfor_val_tag,
        side_trace_parent,
        entry_side,
        self_link_kind,
        retfs,
        downrec_close,
        field_ic_snapshot,
        result_tags,
        field_slots,
        index_slots,
        index_key,
        for_step_up,
        closed_at_head,
        returned_from_head,
        link_pc,
    } = src;
    out.head_proto = *head_proto;
    out.settings = *settings;
    out.head_pc = *head_pc;
    out.entry_tags.clone_from(entry_tags);
    out.ops.clone_from(ops);
    out.closed = *closed;
    out.is_call_triggered = *is_call_triggered;
    out.tfor_iter = *tfor_iter;
    out.tfor_val_tag = *tfor_val_tag;
    (out.side_trace_parent, out.entry_side) = (*side_trace_parent, *entry_side);
    out.self_link_kind = *self_link_kind;
    out.retfs.clone_from(retfs);
    out.downrec_close = *downrec_close;
    out.field_ic_snapshot = *field_ic_snapshot;
    out.result_tags.clone_from(result_tags);
    out.field_slots.clone_from(field_slots);
    out.index_slots.clone_from(index_slots);
    out.index_key = *index_key;
    out.for_step_up = *for_step_up;
    (out.returned_from_head, out.closed_at_head) = (*returned_from_head, *closed_at_head);
    out.link_pc = *link_pc;
    out
}

/// Keeps `r`'s buffers for the next [`copy_record`] on this thread.
pub(in crate::jit_backend::trace) fn give_record(r: TraceRecord) {
    SPARE_RECORD.with(|s| *s.borrow_mut() = Some(r));
}
