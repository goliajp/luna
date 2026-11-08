//! Traces taken up ahead of their loop heads.
//!
//! The first trace a function takes up from another Vm brings the traces of
//! the function's other loop heads with it, so that the code of all of them
//! is placed in one batch and turns executable at once. They are held
//! beside the function, not among its traces (a head has one trace), until
//! the interpreter first takes a back-edge to their head: there, with the
//! registers that will enter it, a trace whose entry tags admit them is
//! installed and run, and one whose tags do not is dropped, the head then
//! counting toward recording, or taking up another trace, as any other.
//!
//! Each head's trace was the first cached of those starting there, as there
//! are no registers to choose by when it is taken up. The code of a trace
//! dropped stays in the Vm's code memory, which is freed only as a whole.
//!
//! The first back-edge reaches the head's check without counting up to the
//! threshold: its slot in the loop counts is set to the threshold. Another
//! loop whose head shares that slot may reach the threshold first; it then
//! starts recording (or taking up a trace) that much sooner, and the held
//! head counts from zero again, its check made once it is hot.

use super::cache::keep_inlined;
use super::*;
use crate::jit::trace::AdoptedTrace;
use crate::runtime::Proto;
use crate::vm::jit_state::loop_hot_slot;

impl Vm {
    /// Holds `later`, the traces taken up for loop heads of `proto` other
    /// than the one asked about, until their heads are reached.
    pub(in crate::vm::exec) fn trace_hold_ahead(
        &mut self,
        proto: Gc<Proto>,
        later: Vec<AdoptedTrace>,
    ) {
        if later.is_empty() {
            return;
        }
        let mut heads: Vec<Vec<AdoptedTrace>> = Vec::new();
        for a in later {
            // the functions they inlined live as long as `proto` from now
            // on, as those of the traces it runs
            keep_inlined(proto, a.inlined.iter().copied());
            match heads.last_mut() {
                Some(h) if a.side_parent.is_some() => h.push(a),
                _ => heads.push(vec![a]),
            }
        }
        self.heap.barrier_back(proto);
        for h in &heads {
            let slot = loop_hot_slot(&proto, h[0].trace.head_pc);
            let threshold = self.jit.trace_hot_threshold;
            // while hot counting pauses the loop counts are parked
            match (&self.jit.step.quiet, &mut self.jit.step.parked_hot) {
                (Some(_), Some(parked)) => parked[slot] = threshold,
                _ => self.jit.loop_hot[slot] = threshold,
            }
        }
        self.jit.counters.ahead_held += heads.len() as u64;
        proto.trace_ahead.borrow_mut().heads.extend(heads);
    }

    /// A back-edge to `target` of `proto` (frame base `base`) is hot: when
    /// a trace is held for that head, installs it if its entry tags admit
    /// the registers now, or drops it. `Some(true)` when it installed it,
    /// `Some(false)` when it dropped it, `None` when none was held.
    pub(in crate::vm::exec) fn trace_take_ahead(
        &mut self,
        proto: Gc<Proto>,
        target: u32,
        base: usize,
    ) -> Option<bool> {
        let held = {
            let mut ahead = proto.trace_ahead.borrow_mut();
            let i = ahead
                .heads
                .iter()
                .position(|h| h[0].trace.head_pc == target)?;
            ahead.heads.swap_remove(i)
        };
        // a side trace starting at the head came first: it keeps the head
        if proto.traces.borrow().iter().any(|t| t.head_pc == target) {
            return None;
        }
        let admits = self
            .stack
            .get(base..base + proto.max_stack as usize)
            .is_some_and(|regs| {
                let now: Vec<u8> = regs.iter().map(|v| v.unpack().0).collect();
                crate::jit::trace_types::adopt::entry_tags_admit(
                    &held[0].trace.plans.entry_tags,
                    &now,
                )
            });
        if !admits {
            self.jit.counters.ahead_dropped += 1;
            return Some(false);
        }
        Some(self.trace_install_adopted(proto, held))
    }
}
