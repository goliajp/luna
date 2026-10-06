//! Leaving a trace through a side trace that started at an exit inside a
//! function the trace inlined: the side trace ran on the registers of the
//! frame that exit rebuilt.

use super::*;
use crate::jit::trace::{CompiledTrace, ExitTag, decode_exit_shape};

impl Vm {
    /// The trace `ct` (head frame at `base_us`, `pre_frames` frames before
    /// it ran) returned `raw_ret` from an exit inside an inlined function,
    /// and `src` names the side trace that then ran from that exit's frame.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_exit_restore_inner(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        ct: &CompiledTrace,
        raw_ret: u64,
        src: ExitSource,
        pre_frames: usize,
        reg_state: Vec<i64>,
        entry_tags: Vec<u8>,
    ) {
        let ExitSource {
            child, body, off, ..
        } = src;
        let child = child.expect("a side trace ran");
        let parent = decode_exit_shape(
            raw_ret,
            &ct.per_exit_inline,
            &ct.per_exit_tags,
            &ct.exit_tags,
        );
        let own = decode_exit_shape(
            body,
            &child.per_exit_inline,
            &child.per_exit_tags,
            &child.exit_tags,
        );
        if let Some(c) = child.exit_hit_counts.get(own.exit_hit_idx) {
            c.set(c.get().saturating_add(1));
        }
        // the exit that rebuilt frames for the side trace to start in
        let site = &ct.per_exit_inline[(parent.site_id - 1) as usize];
        let child_frame = pre_frames - 1 + site.chain.len();
        let tags = composed_exit_tags(
            parent.exit_tags_for_pc,
            own.exit_tags_for_pc,
            off,
            child.entry_tags.len(),
        );
        let keep = match &self.frames[child_frame] {
            CallFrame::Lua(f) => keep_tfor_vars(&f.closure.proto, body, own.cont_pc, off),
            CallFrame::Cont(_) => unreachable!("the side trace's frame is a Lua frame"),
        };
        self.trace_restore_slots(
            base_us,
            cl.proto.max_stack as usize,
            keep,
            false,
            crate::jit::trace::TagResKind::Mixed,
            &tags,
            &reg_state,
            &entry_tags,
            false,
        );
        // each frame resumes after the call that entered the next one; the
        // innermost at the side trace's exit
        if let CallFrame::Lua(f) = &mut self.frames[pre_frames - 1] {
            f.pc = site.head_resume_pc;
        }
        if own.site_id > 0
            && let CallFrame::Lua(f) = &mut self.frames[child_frame]
        {
            f.pc = child.per_exit_inline[(own.site_id - 1) as usize].head_resume_pc;
        }
        if let Some(CallFrame::Lua(f)) = self.frames.last_mut() {
            f.pc = own.cont_pc;
        }
        self.jit.reg_state_buf = reg_state;
        self.jit.entry_tags_buf = entry_tags;
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
