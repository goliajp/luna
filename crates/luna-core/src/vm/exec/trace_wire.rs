//! Hooking a side trace into the exit of its parent trace, whether it was
//! compiled here or taken over from another Vm.

use super::*;
use crate::jit::trace::CompiledTrace;

/// A side trace that passed the shape gate: the parent trace to hold it
/// once it is cached, and the parent's exit it runs from.
pub(super) type Wired = (TArc<CompiledTrace>, u32);

impl Vm {
    /// A closed side trace: hook its entry into the parent trace's exit.
    /// `Some` when it may run from there; [`Self::hold_side_trace`] then
    /// gives the cached trace to the parent.
    pub(super) fn wire_side_trace(
        &mut self,
        ct: &mut CompiledTrace,
        side_trace_parent: Option<(Gc<crate::runtime::function::Proto>, u32, usize)>,
    ) -> Option<Wired> {
        // A side trace is entered only through its parent's exit, never by
        // the dispatcher's lookup at its head pc.
        let (parent_proto, parent_head_pc, parent_exit_idx) = side_trace_parent?;
        // The lowerer's own verdict: a trace it compiled but would not
        // dispatch (an untyped table read, an inline abort) is not safe to
        // enter from the parent's exit either, unless the only reason was
        // the length gate, which weighs dispatch overhead, not soundness
        // (the lowerer records the other reasons first).
        let runnable = ct.dispatchable || ct.dispatch_off_reason == Some("length-gate");
        ct.dispatchable = false;
        let entry_ptr = ct.entry as *const () as *const u8;
        let parent_traces = parent_proto.traces.borrow();
        let parent_ct = parent_traces.iter().find(|t| t.head_pc == parent_head_pc)?;
        // The child's registers are the parent's from the frame the exit
        // resumes in. Its entry tags must match what the exit leaves there,
        // or its code would misread the raw bits the parent wrote. A child
        // that reads a slot the parent neither checked on entry nor writes
        // finds its value on the stack: the dispatcher checks that slot's
        // tag before running the child (`child_reads_stack_held_ok`).
        let off = parent_ct.exit_frame_offset(parent_exit_idx);
        let exit_tags = parent_ct.exit_tags_of(parent_exit_idx);
        // slots past the child's frame belong to frames that are gone when
        // it runs; it neither reads nor writes them
        let end = (off + ct.entry_tags.len()).min(exit_tags.len());
        let shape_matches = exit_tags.len() >= off
            && crate::jit::trace::exit_tags_match_entry_tags(
                &ct.entry_tags,
                &exit_tags[off..end],
                &stack_held_as_child(
                    &parent_ct.entry_tags,
                    &parent_ct.body_writes,
                    &ct.entry_tags,
                    off,
                ),
            );
        if !shape_matches {
            self.jit.counters.side_trace_shape_mismatch += 1;
        }
        if !(runnable && shape_matches) {
            return None;
        }
        // a child that may move to the optimizing tier updates these cells
        // then
        let remember = |k: usize, cell: &crate::jit::send_compat::TCellPtr| {
            if let Some(t) = &ct.tier_up {
                t.parent_cells[k].set(cell as *const _ as *const u8);
            }
        };
        if let Some(cell) = parent_ct.exit_side_trace_ptrs.get(parent_exit_idx) {
            cell.set(entry_ptr);
            remember(0, cell);
        }
        // the per-kind cell, laid out as `exit_hit_counts`: inline exits
        // first, then per_exit_tags, then the global tail slot
        let inline_n = parent_ct.per_exit_inline.len();
        let tags_n = parent_ct.per_exit_tags.len();
        let (sent_kind, sent_local) = if parent_exit_idx < inline_n {
            let cell = &parent_ct.per_exit_inline[parent_exit_idx].side_trace_ptr;
            cell.set(entry_ptr);
            remember(1, cell);
            (
                crate::jit::trace::SIDE_SENT_KIND_INLINE,
                parent_exit_idx as u32,
            )
        } else if parent_exit_idx < inline_n + tags_n {
            let local = parent_exit_idx - inline_n;
            if let Some(b) = parent_ct.tags_side_trace_ptrs.get(local) {
                b.set(entry_ptr);
                remember(1, b);
            }
            (crate::jit::trace::SIDE_SENT_KIND_TAG, local as u32)
        } else {
            parent_ct.global_side_trace_ptr.set(entry_ptr);
            remember(1, &parent_ct.global_side_trace_ptr);
            (crate::jit::trace::SIDE_SENT_KIND_GLOBAL, 0)
        };
        self.jit.counters.side_trace_compiled += 1;
        // the dispatcher decodes the parent's exits for a side trace to run
        // from now on
        parent_ct.has_any_side_wired.set(true);
        let sentinel = crate::jit::trace::encode_side_sentinel(sent_kind, sent_local);
        parent_ct
            .side_trace_cache
            .borrow_mut()
            .insert(sentinel, parent_exit_idx as u32);
        Some((parent_ct.clone(), parent_exit_idx as u32))
    }
}

/// Hands the cached side trace `child` to the parent `wired` names.
pub(super) fn hold_side_trace(wired: Option<Wired>, child: TArc<CompiledTrace>) {
    if let Some((parent, exit)) = wired {
        parent.side_children.borrow_mut().insert(exit, child);
    }
}

/// The parent's entry tags from register `off` on, as the shape gate
/// compares them with the child's: a slot the parent does not check on
/// entry and never writes takes the child's tag, which the dispatcher
/// checks against the stack before it runs the child.
fn stack_held_as_child(
    parent_entry: &[u8],
    parent_writes: &[u32],
    child_entry: &[u8],
    off: usize,
) -> Vec<u8> {
    parent_entry
        .iter()
        .enumerate()
        .skip(off)
        .map(|(i, &t)| {
            if t == crate::jit::trace::ENTRY_TAG_ANY && !parent_writes.contains(&(i as u32)) {
                child_entry.get(i - off).copied().unwrap_or(t)
            } else {
                t
            }
        })
        .collect()
}
