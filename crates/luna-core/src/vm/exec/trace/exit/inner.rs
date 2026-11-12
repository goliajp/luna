//! Leaving a trace through a side trace that started at an exit inside a
//! function the trace inlined: the side trace ran on the registers of the
//! frame that exit rebuilt.

use super::*;
use crate::jit::trace::{CompiledTrace, ExitTag};

impl Vm {
    /// The trace `ct` (head frame at `base_us`, `pre_frames` frames before
    /// it ran) returned `raw_ret` from an exit inside an inlined function,
    /// and `src` names the side trace that then ran from that exit's frame.
    /// `Some` when a frame of the side trace's own inline exit could not be
    /// made (see [`Self::trace_push_unmade_frames`]).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::vm::exec) fn trace_exit_restore_inner(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        ct: &CompiledTrace,
        raw_ret: u64,
        src: ExitSource,
        pre_frames: usize,
        reg_state: &[i64],
        entry_tags: &[u8],
    ) -> Option<(LuaError, u32)> {
        let ExitSource {
            child, body, off, ..
        } = src;
        let child = child.expect("a side trace ran");
        let parent = ct.plans.decode(raw_ret);
        let own = child.plans.decode(body);
        bump(child.exit_hit_counts.get(own.exit_hit_idx));
        // the exit that rebuilt frames for the side trace to start in
        let site = &ct.per_exit_inline[(parent.site_id - 1) as usize];
        let child_frame = pre_frames - 1 + site.chain.len();
        let tags = composed_exit_tags(
            parent.exit_tags_for_pc,
            own.exit_tags_for_pc,
            off,
            child.plans.entry_tags.len(),
        );
        let at = (off, base_us, cl.proto.max_stack as usize);
        let failed = self.unmade_frames_at((&child, own.site_id), (&tags, None), at, reg_state);
        let keep = match &self.frames[child_frame] {
            CallFrame::Lua(f) => keep_tfor_vars(&f.closure.proto, body, own.cont_pc, off),
            CallFrame::Cont(_) => unreachable!("the side trace's frame is a Lua frame"),
        };
        self.trace_restore_slots(
            base_us,
            at.2,
            keep.clone(),
            false,
            crate::jit::trace::TagResKind::Mixed,
            &tags,
            reg_state,
            entry_tags,
            false,
        );
        self.trace_restore_back_edge_kinds(base_us, ct, &tags, keep, reg_state);
        let head_resume = site.head_resume_pc;
        self.resume_inner_frames(pre_frames, child_frame, head_resume, &child, &own);
        failed
    }
}

impl Vm {
    /// Each frame resumes after the call that entered the next one: the
    /// trace's (`pre_frames - 1`) at `head_resume`, the side trace's
    /// (`child_frame`) after its own inlined exit if `own` is one, the
    /// innermost at the side trace's exit.
    fn resume_inner_frames(
        &mut self,
        pre_frames: usize,
        child_frame: usize,
        head_resume: u32,
        child: &CompiledTrace,
        own: &crate::jit::trace::DecodedExit<'_>,
    ) {
        if let CallFrame::Lua(f) = &mut self.frames[pre_frames - 1] {
            f.pc = head_resume;
        }
        if own.site_id > 0
            && let CallFrame::Lua(f) = &mut self.frames[child_frame]
        {
            f.pc = child.per_exit_inline[(own.site_id - 1) as usize].head_resume_pc;
        }
        if let Some(CallFrame::Lua(f)) = self.frames.last_mut() {
            f.pc = own.cont_pc;
        }
    }
}

/// The tags to write the registers back with after a side trace whose
/// register 0 is the parent's `off` ran from the parent's exit that left
/// `parent`: the side trace's where it wrote, the parent's where it did
/// not. Past the side trace's frame (`frame` registers) only what the side
/// trace wrote is live (in frames of functions it inlined); the rest is
/// written nil.
fn composed_exit_tags(
    parent: &[ExitTag],
    child: &[ExitTag],
    off: usize,
    frame: usize,
) -> Vec<ExitTag> {
    let mut tags = parent.to_vec();
    let end = off + child.len().max(frame);
    if tags.len() < end {
        tags.resize(end, ExitTag::Untouched);
    }
    for (i, t) in tags.iter_mut().enumerate().skip(off) {
        let k = i - off;
        match child.get(k) {
            Some(&c) if c != ExitTag::Untouched => *t = c,
            _ if k >= frame => *t = ExitTag::Nil,
            _ => {}
        }
    }
    tags
}

/// One more hit of an exit, saturating.
fn bump(c: Option<&crate::jit::send_compat::TCellU32>) {
    if let Some(c) = c {
        c.set(c.get().saturating_add(1));
    }
}
