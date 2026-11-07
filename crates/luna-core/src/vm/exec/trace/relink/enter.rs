//! Taking a link at run time: what the dispatcher does before it enters a
//! trace, done for the trace a side trace links to (see `onward`) by the
//! parent's code through `luna_jit_side_cell_list`.

use super::*;
use crate::jit::send_compat::TArc;
use crate::jit::trace::CompiledTrace;
use crate::jit::trace_types::frame::trace_ran_side_slot;
use crate::jit::trace_types::side_cell::LINK_DEPTH;
use crate::vm::exec::trace::dispatch::words;

impl Vm {
    /// Enters, for the running trace's code, the trace its side trace links
    /// to: `link` is `[typed exit, running trace, linked trace]` (the traces
    /// by their index in the frame's prototype's list) and `regs` the
    /// buffer both run on. The entry of the linked trace, or 0 when the code
    /// leaves the link to the dispatcher: `LINK_DEPTH` links taken in a row,
    /// a check before the entry failing, or no room to note it.
    pub(super) fn link_enter(&mut self, link: &[u32], regs: &mut [i64]) -> i64 {
        match self.link_take(link, regs) {
            Some(t) => {
                let calls = match self.frames.last() {
                    Some(CallFrame::Lua(f)) => f.closure.proto.call_hot_count.get(),
                    _ => 0,
                };
                self.trace_entry_counted(&t, calls) as i64
            }
            None => 0,
        }
    }

    /// [`Self::link_enter`] but for the entry: the linked trace, readied to
    /// be called.
    pub(in crate::vm::exec) fn link_take(
        &mut self,
        link: &[u32],
        regs: &mut [i64],
    ) -> Option<TArc<CompiledTrace>> {
        let (typed, from, to) = (link[0], link[1] as usize, link[2] as usize);
        let jit = &self.jit;
        if jit.link_trail.len() - jit.link_base >= LINK_DEPTH
            || jit.gc_exit.is_some()
            || jit.pending_err.is_some()
            || self.unmade.any
        {
            return None;
        }
        let Some(CallFrame::Lua(f)) = self.frames.last() else {
            return None;
        };
        let (proto, base) = (f.closure.proto, f.base);
        let (prev, target) = {
            let traces = proto.traces.borrow();
            (TArc::clone(&traces[from]), TArc::clone(&traces[to]))
        };
        debug_assert_eq!(
            TArc::as_ptr(&prev) as usize,
            self.trace_step_running(),
            "a link taken by a trace that is not running"
        );
        let (_, cell) = prev.side_cells.iter().find(|(e, _)| *e == typed)?;
        let lists = cell.link_lists.borrow();
        if self.side_cell_list(&lists.1, regs) == 0
            || !self.inlined_calls_fit(target.inline_depth, base, target.window_size)
            || self
                .jit
                .link_trail
                .push([typed, from as u32, to as u32])
                .is_err()
        {
            return None;
        }
        self.side_cell_list(&lists.2, regs);
        drop(lists);
        let (max_stack, window) = (proto.max_stack as usize, target.window_size as usize);
        words::count_out(&prev, regs, prev.window_size as usize);
        // registers past the frame are an inlined function's
        if window > max_stack {
            regs[max_stack..window].fill(0);
        }
        words::count_in(&target, regs, window);
        if !target.side_cells.is_empty() {
            regs[trace_ran_side_slot(window)] = 0;
        }
        // the dispatcher puts back the window it opened when the call returns
        let _ = self.trace_step_enter(TArc::as_ptr(&target));
        Some(target)
    }
}
