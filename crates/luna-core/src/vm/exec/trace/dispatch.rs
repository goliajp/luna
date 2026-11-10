//! The trace JIT dispatcher: entering a compiled trace at the pc the
//! interpreter is about to run, and resuming the interpreter where it exits.

use super::*;
use crate::runtime::mem::LVec;
use downrec::downrec_close_exit;

mod entry;
mod linked;
mod run;
pub(in crate::vm::exec) mod words;

impl Vm {
    /// Whether a trace may start at `pc` of `proto`: `pc` is one of its
    /// `trace_heads`, or they are too many to list. Elsewhere no trace
    /// admits an entry ([`Self::trace_admits`] wants the trace's head, and
    /// every trace it admits is listed there when cached), so
    /// [`Self::trace_dispatch`] is not called there.
    #[inline(always)]
    pub(in crate::vm::exec) fn trace_head_at(
        proto: &crate::runtime::Proto,
        pc: u32,
        blocked: bool,
    ) -> bool {
        use crate::runtime::function::TRACE_HEADS_MANY;
        let heads = proto.trace_heads.get();
        let at = heads[0] == TRACE_HEADS_MANY || heads.contains(&pc);
        debug_assert!(
            at || !proto
                .traces
                .borrow()
                .iter()
                .any(|t| Vm::trace_admits(t, pc, blocked)),
            "a trace starts at pc {pc}, which is not among the trace heads"
        );
        at
    }

    /// Run the trace cached for `pc` of `cl` (frame `base`), if one admits this
    /// entry. `true` when the trace ran and the frame's pc now points past it.
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_dispatch(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        base: u32,
        downrec_admit_blocked: bool,
    ) -> bool {
        let Some(ct) = ({
            let traces = cl.proto.traces.borrow();
            traces
                .iter()
                .find(|t| Vm::trace_admits(t, pc, downrec_admit_blocked))
                .cloned()
        }) else {
            return false;
        };
        if self.jit.active_trace.is_some() {
            self.trace_link_at(cl.proto, pc);
        }
        // `ct` is held across the dispatch, so its fields are borrowed
        let head_pc_val = ct.head_pc;
        let window_size = ct.window_size;
        let max_stack = cl.proto.max_stack as usize;
        let window_size_us = window_size as usize;
        let base_us = base as usize;
        // the Vm's buffers, taken for the dispatch and put back on every
        // way out; `reg_state` covers the trace's window, which may exceed
        // the frame
        let mem = self.heap.mem();
        let mut entry_tags = std::mem::replace(&mut self.jit.entry_tags_buf, LVec::new(mem));
        // a trace that closes by down-recursion (whether or not it is also
        // dispatchable) guards on the caller's saved pc, in the word just
        // past the window
        let is_downrec_entry = ct.downrec_link.is_some();
        let mut reg_state = std::mem::replace(&mut self.jit.reg_state_buf, LVec::new(mem));
        let dispatch_ok = words::tags_in(&mut entry_tags, max_stack)
            && words::frame_in(&mut reg_state, window_size_us, max_stack)
            && self.inlined_calls_fit(ct.inline_depth, base, ct.window_size)
            && self.trace_marshal_in(
                &ct,
                base_us,
                &mut entry_tags[..max_stack],
                &mut reg_state[..max_stack],
            );

        let mut ran = false;
        if dispatch_ok {
            debug_assert_eq!(head_pc_val, pc, "trace cache hit's head_pc != pc");
            // only an entry that runs counts towards the optimizing tier:
            // one refused would have it compile code for entries that never
            // come
            let entry_fn = self.trace_entry_counted(&ct, cl.proto.call_hot_count.get());
            self.trace_drop_at_compiled();
            self.jit.clear_pending_err();
            // an inline side exit pushes the inlined frames, which a deopt
            // pops
            let pre_frames = self.frames.len();
            if is_downrec_entry {
                reg_state[window_size_us] = self.downrec_saved_pc(pre_frames);
            }
            // `LUNA_AOT_PROBE`: one line at the first dispatch, for the AOT
            // smoke test to see compiled code ran
            if jit_probe_enabled() && self.jit.counters.dispatched == 0 {
                eprintln!("luna-runtime-helpers: aot_trace_fired pc={head_pc_val}");
            }
            self.jit_ghosts_enter();
            words::count_in(&ct, &mut reg_state, window_size_us);
            let (continuation_pc, ct) = self.trace_run(
                cl,
                ct,
                entry_fn,
                &mut reg_state,
                &mut entry_tags[..max_stack],
            );
            debug_assert!(self.jit.blocked_agrees(self.unmade.any()));
            words::count_out(&ct, &reg_state, ct.window_size as usize);
            self.jit.ghost_depth -= 1;
            self.trace_count_dispatch(&ct, is_downrec_entry);
            if is_downrec_entry && downrec_close_exit(continuation_pc) {
                // a due step leaves through an exit, never the close
                debug_assert!(self.jit.gc_exit().is_none());
                self.trace_downrec_classify(pre_frames);
                ran = true;
            } else {
                self.trace_exit_restore(
                    cl,
                    base,
                    &ct,
                    continuation_pc,
                    pre_frames,
                    &mut reg_state,
                    &entry_tags[..max_stack],
                );
                ran = true;
            }
            self.jit.clear_pending_err();
            debug_assert!(!self.unmade.any(), "an exit left something unmade");
        }
        // the buffers back for the next dispatch, before a collection check
        // runs code that may dispatch
        self.jit.reg_state_buf = reg_state;
        self.jit.entry_tags_buf = entry_tags;
        // an error raised at the exit comes first: the op raising it
        // checks for no collection
        if let Some(op_pc) = self.jit.take_gc_exit()
            && self.jit.pending_raise.is_none()
        {
            self.trace_gc_exit_check(op_pc);
        }
        if !self.jit.step.held_later.is_empty() {
            self.trace_hold_later();
        }
        ran
    }
}
