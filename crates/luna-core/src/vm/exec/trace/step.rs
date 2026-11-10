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
    /// The number of frames, its own on top, the running root trace (in
    /// the run words, where the code taking a link writes it) runs with.
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
    /// What [`Vm::trace_hold`] left for the end of the outermost trace run.
    pub(crate) held_later: Vec<super::hold_later::HeldLater>,
}

/// What [`Vm::trace_step_enter`] replaced: the trace, its frames and its
/// frame's base in the run words.
pub(in crate::vm::exec) type RunningTrace = (*const u8, u32, u32);

impl Vm {
    /// A root trace `ct` is about to run in the frame on top: what was
    /// running before, to put back after it. Only `ct`'s address is kept,
    /// to tell the running trace's exits apart; it is not read through.
    /// The frame's words are written only for a trace with side cells
    /// (`cells`): only the code of their exits and the links they take
    /// read them, and a trace without cells takes no link.
    #[inline(always)]
    pub(in crate::vm::exec) fn trace_step_enter(
        &mut self,
        ct: *const crate::jit::trace::CompiledTrace,
        cells: bool,
    ) -> RunningTrace {
        // entering any trace ends a pause a collection exit started
        if self.jit.step.quiet.is_some() {
            cold!(Vm::gc_quiet_end, self);
        }
        let words = &self.jit.words;
        let prev = (words.running.get(), self.jit.step.frames, words.base.get());
        if cells {
            if let Some(CallFrame::Lua(f)) = self.frames.last()
                && f.base * size_of::<Value>() as u32 != prev.2
            {
                words.base.set(f.base * size_of::<Value>() as u32);
            }
            // the stack's buffer moves as it grows; the Vm only between calls
            words.stack.set(std::ptr::addr_of!(self.stack).cast());
        }
        words.running.set(ct.cast());
        self.jit.step.frames = self.frames.len() as u32;
        prev
    }

    /// The trace the step window is open for (its address; 0 none).
    pub(in crate::vm::exec) fn trace_step_running(&self) -> usize {
        self.jit.words.running.get() as usize
    }

    #[inline(always)]
    pub(in crate::vm::exec) fn trace_step_leave(&mut self, prev: RunningTrace) {
        self.jit.step.frames = prev.1;
        self.jit.words.running.set(prev.0);
        self.jit.words.base.set(prev.2);
    }

    /// Whether the run words say what the Vm does: the flags they stand
    /// for, and the running trace's frame (see `trace_step_enter`). For
    /// debug checks where the code of a trace with side cells reads them.
    pub(in crate::vm::exec) fn run_words_agree(&self) -> bool {
        let w = &self.jit.words;
        let base = match self.frames.last() {
            Some(CallFrame::Lua(f)) => f.base * size_of::<Value>() as u32,
            _ => w.base.get(),
        };
        self.jit.blocked_agrees(self.unmade.any())
            && w.base.get() == base
            && w.stack.get() == std::ptr::addr_of!(self.stack).cast()
    }

    /// The running trace's frame, when a step may run in place there.
    fn step_frame(&self) -> Option<(Gc<LuaClosure>, u32)> {
        if self.trace_step_running() == 0 || self.jit.step.frames as usize != self.frames.len() {
            return None;
        }
        match self.frames.last() {
            Some(CallFrame::Lua(f)) => Some((f.closure, f.base)),
            _ => None,
        }
    }

    /// Compiled code reached the point a step falls due at, after the op
    /// whose check `op_pc` names. With `exit_ret` 0, or outside the running
    /// trace's frame: 1 when the step is due, the trace then leaving through
    /// its exit for the interpreter to check ([`Vm::jit_gc_due`]). Otherwise
    /// `reg_state` holds the registers of the trace's frame as of its exit
    /// `exit_ret`: when due, the step runs here, with the
    /// stack and the frame's pc as that exit leaves them; 0 when the trace
    /// goes on, 2 when it leaves through that exit with the step made.
    ///
    /// # Safety
    /// As [`crate::__private::VmJitAlloc::jit_gc_step`]'s.
    pub(crate) unsafe fn jit_gc_step(
        &mut self,
        op_pc: u32,
        exit_ret: u64,
        reg_state: *const i64,
    ) -> i64 {
        let Some((cl, base)) = self.step_frame().filter(|_| exit_ret != 0) else {
            return i64::from(self.jit_gc_due(op_pc));
        };
        if !self.step_due() || self.jit.pending_err().is_some() {
            return 0;
        }
        let Some(ct) = self.stepping_trace(&cl.proto) else {
            return i64::from(self.jit_gc_due(op_pc));
        };
        let n = ct.window_size as usize + crate::jit::trace_types::frame::TRACE_FRAME_TAIL;
        // SAFETY: the call comes from `ct`, the trace running in the top
        // frame (# Safety), whose register buffer `reg_state` covers its
        // window and the words past it
        let regs = unsafe { std::slice::from_raw_parts(reg_state, n) };
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
        let d = ct.plans.decode(exit_ret);
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
        self.trace_restore_back_edge_kinds(base, ct, d.exit_tags_for_pc, keep, regs);
        d.cont_pc
    }

    /// The trace of `proto` the step window was opened for.
    fn stepping_trace(
        &self,
        proto: &crate::runtime::function::Proto,
    ) -> Option<crate::jit::send_compat::TArc<crate::jit::CompiledTrace>> {
        let ct_addr = self.trace_step_running();
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
