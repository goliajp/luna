//! Leaving a trace through a side trace that the dispatcher ran from an
//! exit in the head frame: the whole exit, in the order
//! [`Vm::trace_exit_restore`] takes it, done in the cold path that ran the
//! side trace, so that the dispatcher's own code stays as it is. The
//! write-back goes by the plan of the side trace's exit once that exit is
//! taken often enough.

use super::super::restore::{exit_value, raw_value};
use super::*;
use crate::jit::trace::CompiledTrace;
use crate::jit::trace_types::installed::exit_plan::PlannedExit;

/// The `off` of an [`ExitSource`] whose exit the cold path finished: the
/// dispatcher hands it to [`Vm::trace_exit_inner_side`], which returns. No
/// register offset is this large.
pub(super) const EXIT_DONE: usize = usize::MAX;

/// The times a side trace's exit is taken before its write-back goes by the
/// plan of that exit: the list of the registers it left untouched costs
/// about as much to make as a few write-backs register by register save.
const AFTER_PLANNED_AT: u32 = 8;

impl Vm {
    /// The rest of the exit of `ct` (head frame at `base_us` of `cl`,
    /// `pre_frames` frames before it ran), which returned `raw_ret`, after
    /// the side trace `child` ran from it on the head frame's registers and
    /// returned `child_ret`: as [`Self::trace_exit_restore`] goes on after
    /// a side trace ran, for an exit that made no step in place (the caller
    /// checks; such an exit pauses hot counting).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_exit_after_side(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        ct: &CompiledTrace,
        raw_ret: u64,
        (child, child_ret): (&CompiledTrace, u64),
        pre_frames: usize,
        reg_state: &[i64],
        entry_tags: &[u8],
    ) {
        let max_stack = cl.proto.max_stack as usize;
        let decoded = child.plans.decode(child_ret);
        let (site_id, cont_pc, exit) = (decoded.site_id, decoded.cont_pc, decoded.exit_hit_idx);
        let tags = decoded.exit_tags_for_pc;
        let failed = self.unmade_frames_at(
            (child, site_id),
            (tags, Some(entry_tags)),
            (0, base_us, max_stack),
            reg_state,
        );
        let tier_exit = self.jit.gc_exit().is_some();
        let starts = self.count_exit_hit(ct, &child.exit_hit_counts, exit, tier_exit, true);
        debug_assert!(!starts, "an exit after a side trace ran starts none");
        let keep = keep_tfor_vars(&cl.proto, child_ret, cont_pc, 0);
        let planned = (child.exit_hit_counts.get(exit))
            .is_some_and(|c| c.get() >= AFTER_PLANNED_AT)
            .then(|| child.plans.exit(exit))
            .flatten()
            .filter(|e| keep.is_empty() && base_us + e.len <= self.stack.len());
        if let Some(e) = planned {
            debug_assert_eq!(e.len, tags.len(), "a side exit's plan of other tags");
            self.trace_restore_after_side(base_us, max_stack, e, reg_state, entry_tags);
        } else {
            self.trace_restore_slots(
                base_us,
                max_stack,
                keep.clone(),
                false,
                ct.plans.global_tag_res_kind,
                tags,
                reg_state,
                entry_tags,
                true,
            );
        }
        self.trace_restore_back_edge_kinds(base_us, ct, tags, keep, reg_state);
        let raise_at = match self.unmade.any() {
            true => self.trace_exit_unmade_after(base_us, 0, failed),
            false => None,
        };
        if site_id > 0 {
            let at = child.per_exit_inline[(site_id - 1) as usize].head_resume_pc;
            set_head_resume(&mut self.frames, pre_frames, at);
        }
        if let Some(pc) = raise_at {
            set_top_pc(&mut self.frames, pc);
            return;
        }
        self.set_exit_pc(cont_pc, (true, raw_ret, site_id, pre_frames, max_stack));
    }
}

impl Vm {
    /// [`Self::trace_restore_slots`] after a side trace that starts at the
    /// head frame's register 0 ran and left at the exit `e` plans: the
    /// registers it wrote back with their kinds; the ones in the frame of
    /// `max_stack` registers it left untouched with the tag they entered
    /// the parent with (which may have written them), unless the parent
    /// took them unchecked; the ones past the frame it left untouched as
    /// nil. The caller checks that the stack covers `e.len` registers from
    /// `base_us` and that no generic `for` variables are kept.
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_restore_after_side(
        &mut self,
        base_us: usize,
        max_stack: usize,
        e: &PlannedExit,
        reg_state: &[i64],
        entry_tags: &[u8],
    ) {
        let frame = &mut self.stack[base_us..base_us + e.len];
        let regs = &reg_state[..e.len];
        for w in e.written.iter() {
            let s = usize::from(w.slot);
            if let Some(v) = exit_value(w.kind, regs[s]) {
                frame[s] = v;
            }
        }
        let untouched = e.untouched();
        let in_frame = untouched.partition_point(|&s| usize::from(s) < max_stack);
        for &s in &untouched[..in_frame] {
            let s = usize::from(s);
            let tag = entry_tags[s];
            if tag != crate::jit::trace::ENTRY_TAG_ANY {
                frame[s] = raw_value(tag, regs[s]);
            }
        }
        for &s in &untouched[in_frame..] {
            let s = usize::from(s);
            frame[s] = raw_value(crate::runtime::value::raw::NIL, regs[s]);
        }
    }
}

#[cfg(test)]
mod tests;
