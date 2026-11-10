//! The trace JIT dispatcher: entering a compiled trace at the pc the
//! interpreter is about to run, and resuming the interpreter where it exits.

use super::*;
use crate::runtime::mem::LVec;
use downrec::downrec_close_exit;

mod entry;
mod entry_side;
pub(in crate::vm::exec) use entry_side::forget_refusal;
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

    /// Whether the trace [`Self::trace_dispatch`] would try at the loop
    /// head in slot `at` of `proto.trace_heads` (frame `base`) is sure to
    /// refuse the entry the way it refused one before
    /// (`Proto::trace_head_refused`): the fast loop then runs on past the
    /// head instead of handing it to the loop head, which would only find
    /// the same. Only asked while no recording runs and no trap is set,
    /// which the loop head would look at first.
    #[inline(always)]
    pub(in crate::vm::exec) fn trace_refuses_again(
        &self,
        proto: &crate::runtime::Proto,
        at: usize,
        base: u32,
    ) -> bool {
        let r = proto.trace_head_refused[at].get();
        r != crate::runtime::function::TRACE_HEAD_NOT_REFUSED
            && !self.jit.suppress_downrec_admit_once
            && (self.stack.get(base as usize + (r >> 8) as usize))
                .is_some_and(|v| v.unpack().0 == r as u8)
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
        let Some(mut ct) = ({
            let traces = cl.proto.traces.borrow();
            traces
                .iter()
                .find(|t| Vm::trace_admits(t, pc, downrec_admit_blocked))
                .cloned()
        }) else {
            return false;
        };
        if self.jit.active_trace.is_some() {
            cold!(Vm::trace_link_at, self, cl.proto, pc);
        }
        // counted here, where it costs the least, and taken back if the
        // checks refuse the entry (`trace_refused`)
        let mut entry_fn = self.trace_entry_counted(&ct, cl.proto.call_hot_count.get());
        let max_stack = cl.proto.max_stack as usize;
        // the Vm's buffers, taken for the dispatch and put back on every
        // way out; `reg_state` covers the trace's window, which may exceed
        // the frame
        let mem = self.heap.mem();
        let mut entry_tags = std::mem::replace(&mut self.jit.entry_tags_buf, LVec::new(mem));
        let mut reg_state = std::mem::replace(&mut self.jit.reg_state_buf, LVec::new(mem));
        // a refused entry may go to an entry side trace, which then takes
        // the place of `ct`: one place runs the trace entered, so that its
        // code is not made twice over
        let ran = words::tags_in(&mut entry_tags, max_stack)
            && (ct.has_entry_side.get()
                && match cold!(
                    Vm::trace_side_first,
                    self,
                    cl,
                    &ct,
                    base,
                    &mut reg_state,
                    &mut entry_tags
                ) {
                    Some((es, entry)) => {
                        (ct, entry_fn) = (es, entry);
                        true
                    }
                    None => false,
                }
                || words::frame_in(&mut reg_state, ct.window_size as usize, max_stack)
                    && self.inlined_calls_fit(ct.inline_depth, base, ct.window_size)
                    && match self.trace_marshal_in(
                        &ct,
                        base as usize,
                        &mut entry_tags[..max_stack],
                        &mut reg_state[..max_stack],
                    ) {
                        entry::ENTERS => true,
                        r => match cold!(
                            Vm::trace_refused,
                            self,
                            cl,
                            &ct,
                            pc,
                            base,
                            r,
                            &mut reg_state,
                            &mut entry_tags
                        ) {
                            Some((es, entry)) => {
                                (ct, entry_fn) = (es, entry);
                                true
                            }
                            None => false,
                        },
                    });
        if ran {
            self.trace_dispatch_run(cl, ct, entry_fn, pc, base, &mut reg_state, &mut entry_tags);
        }
        self.trace_dispatch_done(reg_state, entry_tags);
        ran
    }

    /// Runs `ct`, whose checks took the frame at `base` into the buffers
    /// `reg_state` and `entry_tags`, at `entry_fn`, and resumes the
    /// interpreter where it left.
    #[inline(always)]
    // the dispatch's state, as the dispatcher holds it
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_dispatch_run(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: crate::jit::send_compat::TArc<crate::jit::trace::CompiledTrace>,
        entry_fn: crate::jit::trace::TraceFn,
        pc: u32,
        base: u32,
        reg_state: &mut LVec<i64>,
        entry_tags: &mut LVec<u8>,
    ) {
        debug_assert_eq!(ct.head_pc, pc, "trace cache hit's head_pc != pc");
        let max_stack = cl.proto.max_stack as usize;
        // a down-recursion trace guards on the caller's saved pc, just past
        // the window
        let is_downrec_entry = ct.downrec_link.is_some();
        // an inline side exit pushes the inlined frames, a deopt pops them
        let pre_frames = self.frames.len();
        self.trace_dispatch_ready(&ct, pc, is_downrec_entry, reg_state);
        let (continuation_pc, ct) =
            self.trace_run(cl, ct, entry_fn, reg_state, &mut entry_tags[..max_stack]);
        debug_assert!(self.jit.blocked_agrees(self.unmade.any()));
        words::count_out(&ct, reg_state, ct.window_size as usize);
        self.jit.ghost_depth -= 1;
        self.trace_count_dispatch(&ct, is_downrec_entry);
        if is_downrec_entry && downrec_close_exit(continuation_pc) {
            // a due step leaves through an exit, never the close
            debug_assert!(self.jit.gc_exit().is_none());
            cold!(Vm::trace_downrec_classify, self, pre_frames);
        } else {
            self.trace_exit_restore(
                cl,
                base,
                &ct,
                continuation_pc,
                pre_frames,
                reg_state,
                &entry_tags[..max_stack],
            );
        }
        self.jit.clear_pending_err();
        debug_assert!(!self.unmade.any(), "an exit left something unmade");
    }

    /// Readies the Vm and the buffer `reg_state` to run `ct` at head `pc`,
    /// a down-recursion trace when `is_downrec_entry`.
    #[inline(always)]
    fn trace_dispatch_ready(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
        pc: u32,
        is_downrec_entry: bool,
        reg_state: &mut LVec<i64>,
    ) {
        let window_size_us = ct.window_size as usize;
        self.trace_drop_at_compiled();
        self.jit.clear_pending_err();
        if is_downrec_entry {
            reg_state[window_size_us] = cold!(Vm::downrec_saved_pc, self, self.frames.len());
        }
        // `LUNA_AOT_PROBE`: a line at the first dispatch for the AOT smoke test
        if jit_probe_enabled() && self.jit.counters.dispatched == 0 {
            cold!(aot_probe_line, pc);
        }
        self.jit_ghosts_enter();
        words::count_in(ct, reg_state, window_size_us);
    }

    /// The end of every dispatch: the buffers `reg_state` and `entry_tags`
    /// back for the next one, then what the trace's exit left to do.
    #[inline(always)]
    pub(super) fn trace_dispatch_done(&mut self, reg_state: LVec<i64>, entry_tags: LVec<u8>) {
        // the buffers back before a collection check runs code that may
        // dispatch
        self.jit.reg_state_buf = reg_state;
        self.jit.entry_tags_buf = entry_tags;
        // an error raised at the exit comes first: the op raising it
        // checks for no collection
        if let Some(op_pc) = self.jit.take_gc_exit()
            && self.jit.pending_raise.is_none()
        {
            cold!(Vm::trace_gc_exit_check, self, op_pc);
        }
        if !self.jit.step.held_later.is_empty() {
            cold!(Vm::trace_hold_later, self);
        }
    }
}

/// The `LUNA_AOT_PROBE` line of the first dispatch, at head `pc`.
#[cold]
#[inline(never)]
fn aot_probe_line(pc: u32) {
    eprintln!("luna-runtime-helpers: aot_trace_fired pc={pc}");
}
