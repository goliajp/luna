//! Running an entered trace's code: straight through for a trace without
//! side cells, whose code takes no link and reads none of the frame's run
//! words; else with the links its code may take kept (see
//! `crate::vm::exec::trace::relink`).

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
        if !ct.has_side_cells() {
            let running = self.trace_step_enter(&*ct, false);
            let r = self.trace_call(cl, entry, regs, None);
            debug_assert_eq!(self.trace_step_running(), TArc::as_ptr(&ct) as usize);
            self.trace_step_leave(running);
            return (r, ct);
        }
        regs[trace_ran_side_slot(ct.window_size as usize)] = 0;
        let running = self.trace_step_enter(&*ct, true);
        let words = &self.jit.words;
        let links_from = words.link_base();
        words.set_link_base(words.trail_len());
        let start = words.link_base();
        let r = self.trace_call(cl, entry, regs, Some((&ct, start)));
        debug_assert!(
            self.running_is_last_linked(cl.proto, &ct, start),
            "the running trace is not the last one linked to"
        );
        self.trace_step_leave(running);
        let last = self.trace_links_taken(cl.proto, links_from, start, entry_tags);
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
            // SAFETY: `entry` is the entry of a compiled trace `cl.proto.traces` keeps alive (the one dispatched, or one of the same list `trace_link_again` entered after it), checked to take this frame's registers as they are; `regs` was sized to at least the trace's window and the three words past it (no narrower than the other's) and filled by `trace_marshal_in`; the guard above pins this Vm and `cl` for the helpers the trace calls
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
