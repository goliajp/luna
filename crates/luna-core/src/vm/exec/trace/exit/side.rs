//! An exit's hit count, and the side trace a hot exit starts.

use super::*;
use crate::jit::trace::CompiledTrace;

impl Vm {
    /// Counts the hit of exit `exit_hit_idx` in `hit_counts` (none for a
    /// `tier_exit`); whether it is the one that starts a side trace.
    pub(super) fn count_exit_hit(
        &self,
        ct: &CompiledTrace,
        hit_counts: &[crate::jit::send_compat::TCellU32],
        exit_hit_idx: usize,
        tier_exit: bool,
        child_ran: bool,
    ) -> bool {
        let Some(c) = hit_counts.get(exit_hit_idx).filter(|_| !tier_exit) else {
            return false;
        };
        let v = c.get();
        if v < u32::MAX {
            c.set(v + 1);
        }
        // After a side trace ran, `exit_hit_idx` is an
        // exit of that child, but a side trace is
        // recorded and wired as one of `head_pc_val`'s
        // exits: it would replace the side trace on the
        // parent's exit of that number, which resumes
        // elsewhere.
        // An exit that has a side trace already (one taken over from
        // another Vm comes with its side traces wired, and counts
        // from zero) starts none.
        v + 1 == crate::jit::trace::HOTEXIT_THRESHOLD
            && !child_ran
            && self.jit.active_trace.is_none()
            && self.jit.trace_enabled
            && !ct.has_side_trace(exit_hit_idx)
    }

    /// Start recording a side trace at `cont_pc`, the exit of the trace at
    /// `head_pc_val` that just became hot.
    pub(super) fn trace_start_side(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        cont_pc: u32,
        head_pc_val: u32,
        exit_hit_idx: usize,
    ) {
        let (resume_base, resume_proto) = match self.frames.last() {
            Some(CallFrame::Lua(f)) => (f.base as usize, f.closure.proto),
            _ => (base_us, cl.proto),
        };
        let resume_max_stack = resume_proto.max_stack as usize;
        let mut side_entry_tags: Vec<u8> = Vec::with_capacity(resume_max_stack);
        // Extend stack if cont_pc's frame window
        // overhangs the current stack len (rare,
        // but inline-pushed frame stack writes
        // only covered the trace's writeback).
        // a stack that cannot grow just starts no side trace here
        if !self.jit_ensure_stack(resume_base + resume_max_stack) {
            return;
        }
        let parent = Some((cl.proto, head_pc_val, exit_hit_idx));
        if self.trace_try_adopt(resume_proto, cont_pc, resume_base, parent, false) {
            return;
        }
        for i in 0..resume_max_stack {
            let (tag, _) = self.stack[resume_base + i].unpack();
            side_entry_tags.push(tag);
        }
        let mut rec = crate::jit::trace::TraceRecord::start_side_trace(
            resume_proto,
            cont_pc,
            side_entry_tags,
            cl.proto,
            head_pc_val,
            exit_hit_idx,
        );
        rec.settings = self.jit.recording_settings();
        self.jit.active_trace = Some(Box::new(rec));
        self.jit.recording_frame_base = self.frames.len() - 1;
        self.jit.counters.side_trace_started += 1;
    }
}
