//! A collection step that falls due inside a trace, run where the trace is
//! without leaving it: the registers are written back as the exit there
//! would write them, the check is the one the interpreter makes after the
//! op, and the trace goes on unless the step did something the trace
//! cannot see past.

use super::*;

/// The trace running now, for [`Vm::jit_gc_step`], and what a step it
/// runs leaves for its exit.
#[derive(Default)]
pub(crate) struct TraceStep {
    /// The running root trace by address (0: none) and the number of
    /// frames, its own on top, it runs with.
    ct: usize,
    frames: u32,
    /// Set by whatever a step does that a trace cannot go on from: the
    /// stack above the top cleared, a finalizer run.
    pub(crate) disturbed: bool,
    /// The trace left after its step: the stack is as the interpreter
    /// finds it, and the exit writes nothing back.
    pub(crate) synced_exit: bool,
    /// Set while hot counting pauses after a collection exit.
    pub(crate) quiet: Option<super::gc_quiet::GcQuiet>,
    /// The loop counts [`crate::vm::JitState::loop_hot`] does not hold:
    /// the real ones during a pause, else the stand-in it uses then.
    pub(crate) parked_hot: Option<super::gc_quiet::HotTable>,
}

/// What [`Vm::trace_step_enter`] replaced.
pub(in crate::vm::exec) type RunningTrace = (usize, u32);

impl Vm {
    /// A root trace `ct` is about to run in the frame on top: what was
    /// running before, to put back after it. Only `ct`'s address is kept,
    /// to tell the running trace's exits apart; it is not read through.
    pub(in crate::vm::exec) fn trace_step_enter(
        &mut self,
        ct: *const crate::jit::trace::CompiledTrace,
    ) -> RunningTrace {
        // entering any trace ends a pause a collection exit started
        if self.jit.step.quiet.is_some() {
            self.gc_quiet_end();
        }
        let st = &mut self.jit.step;
        let prev = (st.ct, st.frames);
        st.ct = ct as usize;
        st.frames = self.frames.len() as u32;
        prev
    }

    pub(in crate::vm::exec) fn trace_step_leave(&mut self, prev: RunningTrace) {
        (self.jit.step.ct, self.jit.step.frames) = prev;
    }

    /// The running trace's frame, when a step may run in place there.
    fn step_frame(&self) -> Option<(Gc<LuaClosure>, u32)> {
        let st = &self.jit.step;
        if st.ct == 0 || st.frames as usize != self.frames.len() {
            return None;
        }
        match self.frames.last() {
            Some(CallFrame::Lua(f)) => Some((f.closure, f.base)),
            _ => None,
        }
    }

    /// The words of the running trace's buffer a step in it reads (its
    /// window and the words past it, see
    /// `crate::__private::trace_count_slot`), or 0 when it cannot run in
    /// place: no trace of this frame runs, or the trace left no exit for it
    /// (`exit_ret` 0).
    pub(crate) fn jit_step_window(&self, exit_ret: u64) -> usize {
        match self.step_frame() {
            Some((cl, _)) if exit_ret != 0 => self.stepping_trace(&cl.proto).map_or(0, |ct| {
                ct.window_size as usize + crate::__private::TRACE_FRAME_TAIL
            }),
            _ => 0,
        }
    }

    /// Compiled code reached the point a step falls due at, after the op
    /// whose check `op_pc` names. Without `regs` (see
    /// [`Vm::jit_step_window`]): 1 when the step is due, the trace then
    /// leaving through its exit for the interpreter to check
    /// ([`Vm::jit_gc_due`]). With them, the registers of the trace's frame
    /// as of its exit `exit_ret`: when due, the step runs here, with the
    /// stack and the frame's pc as that exit leaves them; 0 when the trace
    /// goes on, 2 when it leaves through that exit with the step made.
    ///
    /// # Safety
    /// As [`crate::__private::VmJitAlloc::jit_gc_step`]'s.
    pub(crate) unsafe fn jit_gc_step(
        &mut self,
        op_pc: u32,
        exit_ret: u64,
        regs: Option<&[i64]>,
    ) -> i64 {
        let (Some(regs), Some((cl, base))) = (regs, self.step_frame()) else {
            return i64::from(self.jit_gc_due(op_pc));
        };
        if !self.step_due() || self.jit.pending_err.is_some() {
            return 0;
        }
        let Some(ct) = self.stepping_trace(&cl.proto) else {
            return i64::from(self.jit_gc_due(op_pc));
        };
        let cont_pc = self.step_write_back(&cl.proto, base as usize, &ct, exit_ret, regs);
        let CallFrame::Lua(f) = self.top_frame_raw_mut() else {
            unreachable!("a trace runs in a Lua frame")
        };
        let pc = std::mem::replace(&mut f.pc, cont_pc);
        let stack = (self.stack.as_ptr() as usize, self.stack.len());
        self.jit.step.disturbed = false;
        let r = self.checkpoint_after_jit(op_pc);
        if r.is_ok()
            && !self.jit.step.disturbed
            && stack == (self.stack.as_ptr() as usize, self.stack.len())
            && self.frames.len() == self.jit.step.frames as usize
        {
            if let CallFrame::Lua(f) = self.top_frame_raw_mut() {
                f.pc = pc;
            }
            return 0;
        }
        if let Err(e) = r {
            self.jit.pending_raise = Some(e);
        }
        self.jit.step.synced_exit = true;
        2
    }

    /// Writes the running trace's registers `regs` back to the frame at
    /// `base` as its exit `exit_ret` leaves them; that exit's pc.
    fn step_write_back(
        &mut self,
        proto: &crate::runtime::function::Proto,
        base: usize,
        ct: &crate::jit::trace::CompiledTrace,
        exit_ret: u64,
        regs: &[i64],
    ) -> u32 {
        let d = crate::jit::trace::decode_exit_shape(
            exit_ret,
            &ct.per_exit_inline,
            &ct.plans.per_exit_tags,
            &ct.plans.exit_tags,
        );
        let keep = keep_tfor_vars(proto, exit_ret, d.cont_pc, 0);
        self.trace_restore_slots(
            base,
            proto.max_stack as usize,
            keep.clone(),
            d.using_global_exit_tags,
            ct.plans.global_tag_res_kind,
            d.exit_tags_for_pc,
            regs,
            &[],
            false,
        );
        // the call's count is read, not put back: the trace goes on counting
        let (tags, w) = (d.exit_tags_for_pc, ct.window_size as usize);
        self.trace_restore_back_edge_kinds(base, &ct.plans.per_exit_tags, tags, keep, regs, w);
        d.cont_pc
    }

    /// The trace of `proto` the step window was opened for.
    fn stepping_trace(
        &self,
        proto: &crate::runtime::function::Proto,
    ) -> Option<crate::jit::send_compat::TArc<crate::jit::CompiledTrace>> {
        let ct_addr = self.jit.step.ct;
        let traces = proto.traces.borrow();
        let t = traces
            .iter()
            .find(|t| crate::jit::send_compat::TArc::as_ptr(t) as usize == ct_addr);
        t.cloned()
    }

    fn top_frame_raw_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("a running trace has a frame")
    }
}
