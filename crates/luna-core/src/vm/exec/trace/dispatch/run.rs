//! Running an entered trace's code: straight through for a trace whose
//! code calls no side trace, as one without side cells or with none wired
//! yet (none is wired while a trace runs: see `trace::hold_later`), which
//! takes no link and reads none of the frame's run words; else with the
//! links its code may take kept (see `crate::vm::exec::trace::relink`).

use super::*;
use crate::jit::send_compat::TArc;
use crate::jit::trace::{CompiledTrace, TraceFn};
use crate::jit::trace_types::frame::trace_ran_side_slot;

impl Vm {
    /// Runs `ct` of `cl`, entered at `entry` on the buffers `regs` and
    /// `entry_tags` the dispatch filled for it: what its code returned, and
    /// the trace whose code returned it (the last one linked to, if any),
    /// with `entry_tags` made the tags that trace was entered with.
    #[inline(always)]
    pub(super) fn trace_run(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: TArc<CompiledTrace>,
        entry: TraceFn,
        regs: &mut LVec<i64>,
        entry_tags: &mut [u8],
    ) -> (i64, TArc<CompiledTrace>) {
        if !(ct.has_any_side_wired.get() && ct.has_side_cells()) {
            let running = self.trace_step_enter(&*ct, false);
            let r = self.trace_call(cl, entry, regs, None);
            debug_assert_eq!(self.trace_step_running(), TArc::as_ptr(&ct) as usize);
            self.trace_step_leave(running);
            return (r, ct);
        }
        self.trace_run_linked(cl, ct, entry, regs, entry_tags)
    }

    /// [`Self::trace_run`] for a trace with side cells and a side trace
    /// wired: its code may take links. As hot as the plain run for a loop
    /// whose exits run side traces.
    #[inline(always)]
    fn trace_run_linked(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: TArc<CompiledTrace>,
        entry: TraceFn,
        regs: &mut LVec<i64>,
        entry_tags: &mut [u8],
    ) -> (i64, TArc<CompiledTrace>) {
        regs[trace_ran_side_slot(ct.window_size as usize)] = 0;
        let running = self.trace_step_enter(&*ct, true);
        // the links taken in this dispatch start at the end of the trail,
        // where an outer dispatch's start, if any, is not (an empty trail
        // is where every dispatch starts)
        let w = &self.jit.words;
        let (outer, start) = (w.link_base(), w.trail_len());
        debug_assert!(outer <= start, "links start past the trail");
        if outer != start {
            w.set_link_base(start);
        }
        let r = self.trace_call(cl, entry, regs, Some((&ct, start)));
        debug_assert!(
            self.running_is_last_linked(cl.proto, &ct, start),
            "the running trace is not the last one linked to"
        );
        self.trace_step_leave(running);
        let w = &self.jit.words;
        if w.trail_len() == start {
            // no link taken: one may have been tried
            if w.link_base() != outer {
                w.set_link_base(outer);
            }
            return (r, ct);
        }
        let last = self.trace_links_taken(cl.proto, outer, start, entry_tags);
        (r, last.unwrap_or(ct))
    }

    /// Calls the code at `entry` on `regs` until it returns for good: with
    /// `linked` (the trace entered and where its links start in the trail),
    /// again at each link `Self::trace_link_again` takes.
    #[inline(always)]
    fn trace_call(
        &mut self,
        cl: Gc<LuaClosure>,
        mut entry: TraceFn,
        regs: &mut LVec<i64>,
        linked: Option<(&CompiledTrace, usize)>,
    ) -> i64 {
        let vm_ptr: *mut Vm = self;
        let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
        loop {
            // SAFETY: `entry` is the entry of a compiled trace `cl.proto.traces`, or the entry side traces of one there, keep alive (the one dispatched, or one of `cl.proto.traces` that `trace_link_again` entered after it), checked to take this frame's registers as they are; `regs` was sized to at least the trace's window and the three words past it (no narrower than the other's) and filled by `trace_marshal_in`; the guard above pins this Vm and `cl` for the helpers the trace calls
            let r = unsafe { entry(regs.as_mut_ptr()) };
            let Some((ct, start)) = linked else {
                break r;
            };
            match self.trace_link_again(cl.proto, ct, start, r, regs) {
                Some(next) => entry = next,
                None => break r,
            }
        }
    }
}
