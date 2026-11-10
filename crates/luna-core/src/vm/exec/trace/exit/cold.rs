//! The parts of a trace's exit that seldom run, off the dispatch's hot path
//! (`cold!`).

use super::*;
use crate::vm::exec::trace::exit::decode::ExitSource;

impl Vm {
    /// The write-back of an exit after which a side trace ran from a frame
    /// the trace inlined (`src`).
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_exit_inner_side(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        ct: &CompiledTrace,
        raw_ret: u64,
        src: ExitSource,
        pre_frames: usize,
        reg_state: &mut crate::runtime::mem::LVec<i64>,
        entry_tags: &[u8],
    ) {
        let off = src.off;
        let failed = self.trace_exit_restore_inner(
            cl, base_us, ct, raw_ret, src, pre_frames, reg_state, entry_tags,
        );
        let unmade = self.unmade.any();
        if let Some(pc) = unmade
            .then(|| self.trace_exit_unmade_after(base_us, off, failed))
            .flatten()
        {
            set_top_pc(&mut self.frames, pc);
        }
    }

    /// The trace left for a collection, or after a step it made in place:
    /// hot counting pauses.
    #[cold]
    #[inline(never)]
    pub(super) fn trace_gc_left(&mut self, pre_frames: usize, head_pc: u32) {
        self.jit.step.synced_exit = false;
        self.gc_quiet_from(pre_frames, head_pc);
    }

    /// A collection-check exit: the check the interpreter makes after the
    /// op `op_pc`.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_gc_exit_check(&mut self, op_pc: u32) {
        if let Err(e) = self.checkpoint_after_jit(op_pc) {
            // raised by the op the interpreter goes on from
            self.jit.pending_raise = Some(e);
        }
    }
}

/// The `LUNA_V2C_PROBE` line of an exit setting the frame's pc: `at` is the
/// return, the pc before, the pc after, the inline site, the frames now and
/// before.
#[cold]
#[inline(never)]
pub(super) fn v2c_set_pc_line(
    child_ran: bool,
    at: (u64, u32, u32, u32, usize, usize),
    max_stack: usize,
) {
    let (raw_ret, prev_pc, cont_pc, site_id, frames_len_now, pre_frames) = at;
    eprintln!(
        "[v2c-set-pc] side_ran={} raw={:#018x} prev_pc={} new_cont_pc={} site_id={} frames.len={} pre_frames={} max_stack={}",
        child_ran, raw_ret, prev_pc, cont_pc, site_id, frames_len_now, pre_frames, max_stack,
    );
}

/// An inline exit pushed frames above the head's, which resumes past its
/// call into them at `head_resume_pc`: at the head it would dispatch the
/// trace again.
#[cold]
#[inline(never)]
pub(super) fn set_head_resume(frames: &mut [CallFrame], pre_frames: usize, head_resume_pc: u32) {
    if pre_frames > 0
        && let CallFrame::Lua(f) = &mut frames[pre_frames - 1]
    {
        f.pc = head_resume_pc;
    }
}

/// Moves the frame on top of `frames`, a Lua frame, to `pc`.
#[cold]
#[inline(never)]
pub(super) fn set_top_pc(frames: &mut [CallFrame], pc: u32) {
    if let Some(CallFrame::Lua(f)) = frames.last_mut() {
        f.pc = pc;
    }
}
