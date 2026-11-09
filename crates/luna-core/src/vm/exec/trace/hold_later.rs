//! Side traces made while a trace runs, held by their parents only once
//! none does. A trace no side trace is wired to runs without what links
//! and side cells need (see `dispatch::run`): its dispatch neither clears
//! the word its code notes a side trace run in nor writes the frame's run
//! words, so none may be wired to it, nor a cell of its filled, while it
//! runs. A trace's code gets back into the interpreter through a
//! collection step that runs finalizers; the trace leaves right after such
//! a step, so its code does not reach a cell filled meanwhile, but its
//! dispatch would decode its return as the code of a trace with a side
//! trace wired, reading that word.

use super::wire::{Wired, hold_side_trace};
use super::*;
use crate::jit::trace::CompiledTrace;

/// A cached trace whose parent holds it once no trace runs (see the module
/// comment), with what [`hold_side_trace`] takes.
pub(crate) struct HeldLater {
    wired: Option<Wired>,
    trace: TArc<CompiledTrace>,
    proto: Gc<crate::runtime::Proto>,
}

impl Vm {
    /// [`hold_side_trace`] for the cached trace `trace` of `proto`, and the
    /// calls compiled into the traces whose cells it fills: now when no
    /// trace runs, else when the outermost run ends.
    pub(in crate::vm::exec) fn trace_hold(
        &mut self,
        wired: Option<Wired>,
        trace: TArc<CompiledTrace>,
        proto: Gc<crate::runtime::Proto>,
    ) {
        if self.jit.ghost_depth > 0 {
            let later = HeldLater {
                wired,
                trace,
                proto,
            };
            self.jit.step.held_later.push(later);
            self.jit.counters.held_later += 1;
            return;
        }
        // a trace cached with no parent only fills cells waiting for it
        if wired.is_none() && !super::relink::may_link_late(&trace, &proto) {
            return;
        }
        let words = &self.jit.words;
        for p in hold_side_trace(wired, trace, &mut self.jit.counters.relink, &proto, words) {
            self.trace_compile_calls(&p);
        }
    }

    /// Once the outermost trace run ended: what [`Self::trace_hold`] left
    /// for then. A parent's exit given a side trace already (another made
    /// for it meanwhile) keeps that one.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_hold_later(&mut self) {
        if self.jit.ghost_depth > 0 {
            return;
        }
        for h in std::mem::take(&mut self.jit.step.held_later) {
            if let Some((p, exit)) = &h.wired
                && p.has_side_trace(*exit as usize)
            {
                continue;
            }
            self.trace_hold(h.wired, h.trace, h.proto);
        }
    }

    /// The prototypes of the traces waiting for [`Self::trace_hold_later`].
    pub(in crate::vm::exec) fn mark_held_later(&self, m: &mut crate::runtime::heap::Marker) {
        for h in &self.jit.step.held_later {
            m.mark(h.proto);
        }
    }
}
