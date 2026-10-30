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
}

/// What [`Vm::trace_step_enter`] replaced.
pub(super) type RunningTrace = (usize, u32);

impl Vm {
    /// A root trace `ct` is about to run in the frame on top: what was
    /// running before, to put back after it.
    pub(super) fn trace_step_enter(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
    ) -> RunningTrace {
        let st = &mut self.jit.step;
        let prev = (st.ct, st.frames);
        st.ct = ct as *const _ as usize;
        st.frames = self.frames.len() as u32;
        prev
    }

    pub(super) fn trace_step_leave(&mut self, prev: RunningTrace) {
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

    /// The number of registers a step in the running trace writes back
    /// (its frame's), or 0 when it cannot run in place: no trace of this
    /// frame runs, or the trace left no exit for it (`exit_ret` 0).
    pub(crate) fn jit_step_window(&self, exit_ret: u64) -> usize {
        match self.step_frame() {
            Some((cl, _)) if exit_ret != 0 => cl.proto.max_stack as usize,
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
    pub(crate) fn jit_gc_step(&mut self, op_pc: u32, exit_ret: u64, regs: Option<&[i64]>) -> i64 {
        let (Some(regs), Some((cl, base))) = (regs, self.step_frame()) else {
            return i64::from(self.jit_gc_due(op_pc));
        };
        if !self.step_due() || self.jit.pending_err.is_some() {
            return 0;
        }
        let ct_addr = self.jit.step.ct;
        let found = cl
            .proto
            .traces
            .borrow()
            .iter()
            .find(|t| crate::jit::send_compat::TArc::as_ptr(t) as usize == ct_addr)
            .cloned();
        let Some(ct) = found else {
            return i64::from(self.jit_gc_due(op_pc));
        };
        let d = crate::jit::trace::decode_exit_shape(
            exit_ret,
            &ct.per_exit_inline,
            &ct.per_exit_tags,
            &ct.exit_tags,
        );
        let proto = &cl.proto;
        self.trace_restore_slots(
            base as usize,
            proto.max_stack as usize,
            keep_tfor_vars(proto, exit_ret, d.cont_pc, 0),
            d.using_global_exit_tags,
            ct.global_tag_res_kind,
            d.exit_tags_for_pc,
            regs,
            &[],
            false,
        );
        let CallFrame::Lua(f) = self.top_frame_raw_mut() else {
            unreachable!("a trace runs in a Lua frame")
        };
        let pc = std::mem::replace(&mut f.pc, d.cont_pc);
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

    fn top_frame_raw_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("a running trace has a frame")
    }
}
