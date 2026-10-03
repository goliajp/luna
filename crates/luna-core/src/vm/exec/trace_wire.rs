//! Hooking a side trace into the exit of its parent trace, whether it was
//! compiled here or taken over from another Vm.

use super::*;
use crate::jit::trace::CompiledTrace;

impl Vm {
    /// A closed side trace: hook its entry into the parent trace's exit.
    pub(super) fn wire_side_trace(
        &mut self,
        ct: &mut CompiledTrace,
        side_trace_parent: Option<(Gc<crate::runtime::function::Proto>, u32, usize)>,
        head_proto: Gc<crate::runtime::function::Proto>,
    ) {
        // Side-trace finalisation.
        // Pin `dispatchable=false` so the
        // primary lookup `traces.find(|t|
        // t.head_pc == pc && t.dispatchable)`
        // never matches this entry — the
        // side trace is meant to be entered
        // ONLY through the parent's exit
        // indirection, not the
        // back-edge / call-trigger paths.
        // Then write the entry fn ptr into
        // the parent's `exit_side_trace_ptrs`
        // slot so the parent's IR can read it.
        if let Some((parent_proto, parent_head_pc, parent_exit_idx)) = side_trace_parent {
            // The lowerer's own verdict: a trace it
            // compiled but would not dispatch (an
            // untyped table read, an inline abort)
            // is not safe to enter from the parent's
            // exit either — unless the only reason
            // was the length gate, which weighs
            // dispatch overhead, not soundness (the
            // lowerer records the other reasons
            // first).
            let runnable = ct.dispatchable || ct.dispatch_off_reason == Some("length-gate");
            ct.dispatchable = false;
            let entry_ptr = ct.entry as *const () as *const u8;
            let parent_traces = parent_proto.traces.borrow();
            if let Some(parent_ct) = parent_traces.iter().find(|t| t.head_pc == parent_head_pc) {
                // Shape-match
                // gate. Find the parent's per-exit
                // tag snapshot at the wired exit
                // (inline / tag / global) and
                // check the child's entry_tags
                // match. If not, leave the cell
                // null + skip cache populate so
                // the parent IR's
                // `call_indirect` stays inert at
                // this exit (the child's
                // shape-specialised IR would
                // mis-interpret raw bits the
                // parent writes to reg_state).
                let inline_n = parent_ct.per_exit_inline.len();
                let tags_n = parent_ct.per_exit_tags.len();
                let parent_exit_tags_slice: &[crate::jit::trace::ExitTag] =
                    if parent_exit_idx < inline_n {
                        &parent_ct.per_exit_inline[parent_exit_idx].exit_tags
                    } else if parent_exit_idx < inline_n + tags_n {
                        &parent_ct.per_exit_tags[parent_exit_idx - inline_n].1
                    } else {
                        &parent_ct.exit_tags
                    };
                // A child that reads a slot the parent neither checked on
                // entry nor writes finds its value on the stack: the
                // dispatcher checks that slot's tag before running the child
                // (`child_reads_stack_held_ok`).
                let shape_matches = crate::jit::trace::exit_tags_match_entry_tags(
                    &ct.entry_tags,
                    parent_exit_tags_slice,
                    &stack_held_as_child(
                        &parent_ct.entry_tags,
                        &parent_ct.body_writes,
                        &ct.entry_tags,
                    ),
                );
                if !shape_matches {
                    self.jit.counters.side_trace_shape_mismatch += 1;
                }
                let shape_ok = runnable && shape_matches;
                // Write the child's
                // entry fn ptr to BOTH the legacy
                // `exit_side_trace_ptrs[idx]`
                // cell (read by the
                // walk_any_side_ptr_non_null tests)
                // AND the per-kind cell
                // whose heap address the parent's
                // IR baked. The IR-baked
                // cell is what the call_indirect
                // gate actually reads. Only write
                // when the shape gate passes.
                if shape_ok {
                    // a child that may move to the optimizing tier updates
                    // these cells then
                    let remember = |k: usize, cell: &crate::jit::send_compat::TCellPtr| {
                        if let Some(t) = &ct.tier_up {
                            t.parent_cells[k].set(cell as *const _ as *const u8);
                        }
                    };
                    if let Some(cell) = parent_ct.exit_side_trace_ptrs.get(parent_exit_idx) {
                        cell.set(entry_ptr);
                        remember(0, cell);
                    }
                    // Compute (kind, local) for the
                    // IR-baked cell. Layout follows
                    // exit_hit_counts: inline first,
                    // then per_exit_tags, then the
                    // global tail slot.
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
                    // Flip the
                    // parent's fast-path hint so
                    // the dispatcher knows to do
                    // the tentative decode + cell
                    // check on subsequent
                    // dispatches. Set once and
                    // stays true (we never unwire
                    // a side trace today).
                    parent_ct.has_any_side_wired.set(true);

                    // Populate
                    // the O(1) lookup cache the
                    // dispatcher consults on
                    // sentinel-bit-set returns.
                    // Key is the encoded sentinel
                    // (same encoding the IR ORs
                    // into bits 56..=62 of the
                    // child's i64 return).
                    let sentinel = crate::jit::trace::encode_side_sentinel(sent_kind, sent_local);
                    let predicted_idx = if std::ptr::eq(parent_proto.as_ptr(), head_proto.as_ptr())
                    {
                        parent_traces.len() as u32
                    } else {
                        head_proto.traces.borrow().len() as u32
                    };
                    parent_ct
                        .side_trace_cache
                        .borrow_mut()
                        .insert(sentinel, predicted_idx);
                }
            }
            drop(parent_traces);
        }
    }
}

/// The parent's entry tags as the shape gate compares them: a slot the
/// parent does not check on entry and never writes takes the child's tag,
/// which the dispatcher checks against the stack before it runs the child.
fn stack_held_as_child(parent_entry: &[u8], parent_writes: &[u32], child_entry: &[u8]) -> Vec<u8> {
    parent_entry
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            if t == crate::jit::trace::ENTRY_TAG_ANY && !parent_writes.contains(&(i as u32)) {
                child_entry.get(i).copied().unwrap_or(t)
            } else {
                t
            }
        })
        .collect()
}
