//! Hooking a side trace into the exit of its parent trace, whether it was
//! compiled here or taken over from another Vm.

use super::*;
use crate::jit::trace::CompiledTrace;

/// A side trace that passed the shape gate: the parent trace to hold it
/// once it is cached, and the parent's exit it runs from.
pub(in crate::vm::exec) type Wired = (TArc<CompiledTrace>, u32);

/// The `dispatch_off_reason` of a side trace.
const SIDE_TRACE: &str = "side-trace";

impl Vm {
    /// Whether the interpreter at `pc` enters trace `t`: one headed there
    /// that is dispatchable, or that closes by down-recursion (unless
    /// `downrec_blocked`, set for one entry after a forced exit so the
    /// interpreter runs the op at the head). Never a side trace: it runs
    /// only from its parent's exit, so its parent's function is running or
    /// inlined by a live trace, which tiering the side trace up reads.
    pub(crate) fn trace_admits(t: &CompiledTrace, pc: u32, downrec_blocked: bool) -> bool {
        if t.head_pc != pc || t.dispatch_off_reason == Some(SIDE_TRACE) || downrec_blocked {
            return false;
        }
        // a single-compare down-recursion trace stays undispatchable (most
        // entries would miss its guard) but is entered through its link
        t.dispatchable || t.downrec_link.is_some()
    }
}

impl Vm {
    /// A closed side trace: hook its entry into the parent trace's exit.
    /// `Some` when it may run from there; [`Self::hold_side_trace`] then
    /// gives the cached trace to the parent.
    pub(in crate::vm::exec) fn wire_side_trace(
        &mut self,
        ct: &mut CompiledTrace,
        side_trace_parent: Option<(Gc<crate::runtime::function::Proto>, u32, usize)>,
    ) -> Option<Wired> {
        // A side trace is entered only through its parent's exit, never by
        // the dispatcher's lookup at its head pc.
        let (parent_proto, parent_head_pc, parent_exit_idx) = side_trace_parent?;
        // a trace the lowerer would not dispatch (an untyped table read, an
        // inline abort) is no safer from the parent's exit, unless only the
        // length gate (overhead, not soundness; recorded last) refused it
        // nor is one counting in its call's words, which nothing sets up for
        // a side trace
        let runnable =
            (ct.dispatchable || ct.dispatch_off_reason == Some("length-gate")) && !ct.call_count;
        ct.dispatchable = false;
        ct.dispatch_off_reason = Some(SIDE_TRACE);
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
        let end = (off + ct.plans.entry_tags.len()).min(exit_tags.len());
        let shape_matches = exit_tags.len() >= off
            && crate::jit::trace::exit_tags_match_entry_tags(
                &ct.plans.entry_tags,
                &exit_tags[off..end],
                &stack_held_as_child(
                    &parent_ct.plans.entry_tags,
                    &parent_ct.body_writes,
                    &ct.plans.entry_tags,
                    off,
                ),
            );
        if !shape_matches {
            self.jit.counters.side_trace_shape_mismatch += 1;
        }
        if !(runnable && shape_matches) {
            return None;
        }
        self.jit.counters.side_trace_compiled += 1;
        // the dispatcher decodes the parent's exits for a side trace to run
        // from now on
        parent_ct.has_any_side_wired.set(true);
        Some((parent_ct.clone(), parent_exit_idx as u32))
    }
}

/// Hands the cached side trace `child` to the parent `wired` names.
pub(in crate::vm::exec) fn hold_side_trace(wired: Option<Wired>, child: TArc<CompiledTrace>) {
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
