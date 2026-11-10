//! Leaving a compiled trace: decode which exit it took (running a wired
//! side trace first), write the registers back with their tags and move the
//! frame to the exit's pc.

use super::*;
use crate::jit::trace::CompiledTrace;

pub(in crate::vm::exec) mod decode;
pub(in crate::vm::exec) mod inner;
mod side;
pub(in crate::vm::exec) mod unmade;
mod unmade_frames;

impl Vm {
    /// The trace `ct` entered at `pc` returned `continuation_pc`, its
    /// registers in the dispatcher's buffers `reg_state` and `entry_tags`.
    /// One that left after an allocation with a step due checks for a
    /// collection once the dispatcher has its buffers back
    /// ([`Self::trace_gc_exit_check`]).
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::vm::exec) fn trace_exit_restore(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        ct: &CompiledTrace,
        continuation_pc: i64,
        pre_frames: usize,
        reg_state: &mut crate::runtime::mem::LVec<i64>,
        entry_tags: &[u8],
    ) {
        let synced = self.jit.step.synced_exit;
        if synced || self.jit.gc_exit().is_some() {
            cold!(Vm::trace_gc_left, self, pre_frames, ct.head_pc);
        }
        self.trace_exit_regs(
            cl,
            base,
            ct,
            continuation_pc,
            pre_frames,
            reg_state,
            entry_tags,
            synced,
        );
    }

    /// The write-back of an exit after which a side trace ran from a frame
    /// the trace inlined (`src`).
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn trace_exit_inner_side(
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
    fn trace_gc_left(&mut self, pre_frames: usize, head_pc: u32) {
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

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn trace_exit_regs(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        ct: &CompiledTrace,
        continuation_pc: i64,
        pre_frames: usize,
        reg_state: &mut crate::runtime::mem::LVec<i64>,
        entry_tags: &[u8],
        synced: bool,
    ) {
        let per_exit_inline = &ct.per_exit_inline;
        let plans = &*ct.plans;
        let max_stack = cl.proto.max_stack as usize;
        let base_us = base as usize;
        // the exit's shape (`decode_exit_shape`): the upper 32 bits are the
        // inline side exit's site + 1 (0 for any other exit)
        let raw_ret = continuation_pc as u64;
        let src = self.trace_exit_source(cl, ct, raw_ret, reg_state, base_us, entry_tags);
        if src.ran && src.off > 0 {
            cold!(
                Vm::trace_exit_inner_side,
                self,
                cl,
                base_us,
                ct,
                raw_ret,
                src,
                pre_frames,
                reg_state,
                entry_tags
            );
            return;
        }
        let ExitSource {
            child,
            body: decode_body,
            ran: child_ran,
            ..
        } = src;
        let shapes: &CompiledTrace = child.as_deref().unwrap_or(ct);
        let decode_inline = &shapes.per_exit_inline;
        let decode_hit_counts = &shapes.exit_hit_counts;
        let shape_plans = child.as_deref().map_or(plans, |c| &*c.plans);
        let decoded = shape_plans.decode(decode_body);
        let site_id = decoded.site_id;
        let cont_pc = decoded.cont_pc;
        let exit_hit_idx = decoded.exit_hit_idx;
        let exit_tags_for_pc = decoded.exit_tags_for_pc;
        let tags = (exit_tags_for_pc, child_ran.then_some(entry_tags));
        let failed =
            self.unmade_frames_at((shapes, site_id), tags, (0, base_us, max_stack), reg_state);
        // after a side trace ran, the per-tag path: `global_tag_res_kind`
        // classifies the parent's exit tags, not the child's
        let using_global_exit_tags = if child_ran {
            false
        } else {
            decoded.using_global_exit_tags
        };
        // the exit's hit count (the child's after a side trace), saturating;
        // the one that reaches `HOTEXIT_THRESHOLD` starts a side trace
        // once the stack and pc are restored
        // an exit to check for a collection is no guard failing either: it
        // starts no side trace, which would run past the check
        let tier_exit =
            !child_ran && is_tier_exit(ct, cont_pc) || self.jit.gc_exit().is_some() || synced;
        let side_trace_should_start =
            self.count_exit_hit(ct, decode_hit_counts, exit_hit_idx, tier_exit, child_ran);
        // after a step the trace made in place, the stack is the
        // interpreter's already: a finalizer may have changed it since
        if !synced {
            let keep = keep_tfor_vars(&cl.proto, decode_body, cont_pc, 0);
            let planned = if !child_ran && keep.is_empty() {
                match shape_plans.exit(exit_hit_idx) {
                    Some(e)
                        if e.untouched_end <= max_stack && base_us + e.len <= self.stack.len() =>
                    {
                        Some(e)
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(e) = planned {
                self.trace_restore_planned(base_us, &e.written, reg_state);
            } else {
                self.trace_restore_slots(
                    base_us,
                    max_stack,
                    keep.clone(),
                    using_global_exit_tags,
                    plans.global_tag_res_kind,
                    exit_tags_for_pc,
                    reg_state,
                    entry_tags,
                    child_ran,
                );
            }
            self.trace_restore_back_edge_kinds(base_us, ct, exit_tags_for_pc, keep, reg_state);
        }
        // before anything reads the stack the exit restored
        let unmade = self.unmade.any();
        let raise_at = if unmade {
            cold!(Vm::trace_exit_unmade_after, self, base_us, 0, failed)
        } else {
            None
        };
        let _ = &per_exit_inline; // hold the Rc alive across dispatch
        // an inline exit pushed frames above the head's, which resumes past
        // its call into them: at `head_pc` it would dispatch the trace again
        if site_id > 0 {
            let idx = (site_id - 1) as usize;
            cold!(
                set_head_resume,
                &mut self.frames,
                pre_frames,
                decode_inline[idx].head_resume_pc
            );
        }
        // a frame could not be made: the call into it raises the error
        if let Some(pc) = raise_at {
            cold!(set_top_pc, &mut self.frames, pc);
            return;
        }
        let frames_len_now = self.frames.len();
        // SAFETY: the trace head's Lua frame is still on `frames` (an inline exit only pushed frames above it, and `pre_frames` counts it), so `frames` is not empty
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(fmut) => {
                if crate::jit::trace::v2c_probe_enabled() {
                    let at = (
                        raw_ret,
                        fmut.pc,
                        cont_pc,
                        site_id,
                        frames_len_now,
                        pre_frames,
                    );
                    cold!(v2c_set_pc_line, child_ran, at, max_stack);
                }
                fmut.pc = cont_pc;
            }
            _ => unreachable!("Cont frame at trace dispatch"),
        }
        // Deferred side-trace start. The
        // increment block above flagged this exit's
        // hit count crossing HOTEXIT_THRESHOLD; now
        // that vm.stack is restored and frame.pc is
        // settled, snapshot entry_tags from the
        // resume frame's window and create the
        // recorder. The recorder's first push fires
        // on the next interp iteration at cont_pc.
        //
        // The side trace starts in the resume frame (top of
        // `self.frames`: an exit inside an inlined function rebuilt
        // its frames), which need not run the parent's function.
        if side_trace_should_start {
            cold!(
                Vm::trace_start_side,
                self,
                cl,
                base_us,
                cont_pc,
                ct.head_pc,
                exit_hit_idx
            );
        }
    }
}

/// The `LUNA_V2C_PROBE` line of an exit setting the frame's pc: `at` is the
/// return, the pc before, the pc after, the inline site, the frames now and
/// before.
#[cold]
#[inline(never)]
fn v2c_set_pc_line(child_ran: bool, at: (u64, u32, u32, u32, usize, usize), max_stack: usize) {
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
fn set_head_resume(frames: &mut [CallFrame], pre_frames: usize, head_resume_pc: u32) {
    if pre_frames > 0
        && let CallFrame::Lua(f) = &mut frames[pre_frames - 1]
    {
        f.pc = head_resume_pc;
    }
}

/// Whether `ct` left at `cont_pc` to move to the optimizing tier: that exit
/// is no guard failing, so it neither counts nor starts a side trace. Its
/// count equals `at` only between that exit and the next dispatch, which
/// tiers up first.
#[inline(always)]
fn is_tier_exit(ct: &CompiledTrace, cont_pc: u32) -> bool {
    cont_pc == ct.head_pc
        && ct
            .tier_up
            .as_ref()
            .is_some_and(|t| !t.tried.get() && t.count.get() == t.at)
}

/// Moves the frame on top of `frames`, a Lua frame, to `pc`.
#[cold]
#[inline(never)]
fn set_top_pc(frames: &mut [CallFrame], pc: u32) {
    if let Some(CallFrame::Lua(f)) = frames.last_mut() {
        f.pc = pc;
    }
}
