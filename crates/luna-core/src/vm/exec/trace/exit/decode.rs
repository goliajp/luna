//! Decoding a trace's return: which trace's exit shapes apply (a side
//! trace may have run) and the register write-back with the exit's tags.

use super::*;
use crate::jit::trace::{CompiledTrace, ExitTag};
use crate::jit::trace_types::frame::{TRACE_FRAME_TAIL, trace_ran_side_slot};
use crate::runtime::mem::LVec;

impl Vm {
    /// Which trace's exit shapes decode `raw_ret`: a side trace's when one
    /// is wired to the exit taken (it runs here first), else the trace's
    /// own.
    #[inline(always)]
    pub(in crate::vm::exec) fn trace_exit_source(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: &CompiledTrace,
        raw_ret: u64,
        reg_state: &mut LVec<i64>,
        base_us: usize,
        entry_tags: &[u8],
    ) -> ExitSource {
        let src = match ran_from_code(ct, raw_ret, reg_state) {
            Some(src) => src,
            None if self.exit_runs_no_side(ct) => ExitSource::parent_only(raw_ret),
            None => cold!(
                Vm::exit_source_of,
                self,
                cl,
                ct,
                raw_ret,
                reg_state,
                base_us,
                entry_tags
            ),
        };
        if src.ran {
            self.count_side_pass(cl, ct, &src);
        }
        src
    }

    /// A side trace runs one pass for each return through
    /// [`Self::trace_exit_source`], and the dispatcher never enters it: the
    /// pass counted as its entry.
    #[inline(always)]
    fn count_side_pass(&mut self, cl: Gc<LuaClosure>, ct: &CompiledTrace, src: &ExitSource) {
        if let Some(c) = &src.child {
            self.count_towards_tier_up(c, cl.proto.call_hot_count.get());
            super::super::relink::relink_refresh(ct, src.exit, c);
        }
    }

    #[cold]
    #[inline(never)]
    fn exit_source_of(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: &CompiledTrace,
        raw_ret: u64,
        reg_state: &mut LVec<i64>,
        base_us: usize,
        entry_tags: &[u8],
    ) -> ExitSource {
        let parent_only = ExitSource::parent_only(raw_ret);
        // the side trace wired to the exit taken runs here, not from the
        // parent's code (a test at every exit cost more than it saved)
        let tentative = ct.plans.decode(raw_ret);
        let exit = tentative.exit_hit_idx;
        let Some(child) = ct.side_children.borrow().get(exit).cloned().flatten() else {
            return parent_only;
        };
        // a side trace was recorded from the pc its exit resumed at; a
        // plain-pc exit can return other pcs (a for loop's tail goes back
        // to the body or leaves the loop), and the child only continues
        // from its own
        let off = ct.exit_frame_offset(exit);
        if child.head_pc != tentative.cont_pc
            || !self.child_reads_stack_held_ok(
                base_us,
                off,
                entry_tags,
                ct.exit_tags_of(exit),
                &child.entry_plan,
            )
        {
            return parent_only;
        }
        if !child_frame_in(reg_state, off, &child) {
            return parent_only;
        }
        // The child's registers start at the frame the exit resumes in.
        // Past that frame they are its own scratch slots, which start at
        // zero as on any entry; the parent left nothing there that is
        // still live.
        let child_cl = if off == 0 {
            cl
        } else {
            match self.frames.last() {
                // the exit rebuilt the frames of the functions the parent
                // inlined: the innermost is the one the child starts in
                Some(CallFrame::Lua(f)) => {
                    let frame_end = off + f.closure.proto.max_stack as usize;
                    let need = off + child.window_size as usize;
                    if need > frame_end {
                        reg_state[frame_end..need].fill(0);
                    }
                    f.closure
                }
                _ => return parent_only,
            }
        };
        let cent = child.current_entry();
        self.count_side_trace_run(off);
        let pre_frames = self.frames.len();
        let child_raw_ret = {
            let vm_ptr: *mut Vm = self;
            // code that calls no helper finds nothing through the scope
            let _guard = (child.calls_helpers.get())
                .then(|| self.jit.chunk_compiler.enter(vm_ptr, Some(child_cl)));
            // SAFETY: `cent` is the entry of a side trace compiled for this exit and held by `ct`, whose code lives as long as this Vm; from register `off` on, `reg_state` holds at least the child's window and the three words past it, and the registers the child reads carry the tags it was compiled for (checked when it was wired, and the stack-held ones just above); `child_cl` runs the frame the child starts in, and when the code calls helpers the guard pins it and this Vm for them
            unsafe { cent(reg_state.as_mut_ptr().add(off)) }
        };
        let src = ExitSource {
            child: Some(child),
            body: child_raw_ret as u64,
            ran: true,
            off,
            exit,
        };
        // on the head frame's registers, after an exit that made no step in
        // place (one that did pauses hot counting): the rest of the exit is
        // done here, and the dispatcher only ends it
        if off == 0 && self.jit.step.quiet.is_none() {
            self.count_side_pass(cl, ct, &src);
            let child = src.child.as_deref().expect("a side trace ran");
            let ran = (child, src.body);
            self.trace_exit_after_side(
                cl, base_us, ct, raw_ret, ran, pre_frames, reg_state, entry_tags,
            );
            return ExitSource {
                child: None,
                body: raw_ret,
                ran: true,
                off: super::after_side::EXIT_DONE,
                exit,
            };
        }
        src
    }

    /// Whether the exit `ct` took runs no side trace: none is wired, or
    /// the exit is a collection check (the check is the op's), at an op a
    /// helper could not do, or one whose registers are not all made yet.
    #[inline(always)]
    fn exit_runs_no_side(&self, ct: &CompiledTrace) -> bool {
        !ct.has_any_side_wired.get()
            || self.jit.gc_exit().is_some()
            || self.jit.pending_err().is_some()
            || self.unmade.any()
    }

    fn count_side_trace_run(&mut self, off: usize) {
        self.jit.counters.side_trace_runs += 1;
        if off > 0 {
            self.jit.counters.side_trace_runs_inlined += 1;
        }
    }

    /// Whether a side trace may run on the registers as they are: a slot
    /// the parent took unchecked (its runtime entry tag is ANY) and left
    /// alone by the exit (`at_exit`), but the child checks (`child`, its
    /// entry plan), holds on the stack a value of the tag the child was
    /// compiled for. A slot the exit wrote holds its value in the
    /// registers, not yet on the stack, with the tag wiring checked against
    /// the child's. The child's register `i` is the parent's `off + i`; one
    /// past the frame's tags the parent did not take.
    fn child_reads_stack_held_ok(
        &self,
        base_us: usize,
        off: usize,
        entry_tags: &[u8],
        at_exit: &[ExitTag],
        child: &crate::jit::trace_types::installed::entry_plan::EntryPlan,
    ) -> bool {
        child.checked().iter().all(|e| {
            let i = off + usize::from(e.slot);
            entry_tags
                .get(i)
                .is_none_or(|&p| p != crate::jit::trace::ENTRY_TAG_ANY)
                || at_exit.get(i).is_some_and(|&t| t != ExitTag::Untouched)
                || self.stack[base_us + i].unpack().0 == e.tag
        })
    }
}

/// Sizes `reg_state` to hold `child`'s window and the words past it from
/// `reg_state[off]`, its register 0. A side trace never counts in its
/// call's words (`wire_side_trace` refuses one that does), so it reads none
/// of them, and the parent's registers they may lie on stay as they are.
/// `false` when it cannot grow: the child does not run.
fn child_frame_in(reg_state: &mut LVec<i64>, off: usize, child: &CompiledTrace) -> bool {
    debug_assert!(!child.call_count, "a side trace counting per call");
    let tail = off + child.window_size as usize + TRACE_FRAME_TAIL;
    reg_state.len() >= tail || reg_state.resize(tail, 0).is_ok()
}

/// Which trace's exit decodes a trace's return.
pub(in crate::vm::exec) struct ExitSource {
    /// The side trace whose exit shapes decode the return (`None`: its own).
    pub(in crate::vm::exec) child: Option<TArc<CompiledTrace>>,
    /// The return bits to decode.
    pub(in crate::vm::exec) body: u64,
    /// A side trace ran.
    pub(in crate::vm::exec) ran: bool,
    /// The register of the parent's window the side trace's register 0 is.
    pub(in crate::vm::exec) off: usize,
    /// The parent's exit the side trace ran from (as `exit_hit_counts`
    /// numbers them), when one ran.
    pub(in crate::vm::exec) exit: usize,
}

impl ExitSource {
    /// No side trace ran: the return `raw_ret` decodes with the trace's own
    /// exits.
    #[inline(always)]
    fn parent_only(raw_ret: u64) -> Self {
        ExitSource {
            child: None,
            body: raw_ret,
            ran: false,
            off: 0,
            exit: 0,
        }
    }
}

/// The side trace `ct`'s code ran from a side cell and returned from, when
/// it noted one (see `frame::trace_ran_side_slot`): its return, `raw_ret`,
/// is decoded with its exits. It runs at the exit's frame, the head's.
#[inline(always)]
fn ran_from_code(ct: &CompiledTrace, raw_ret: u64, reg_state: &[i64]) -> Option<ExitSource> {
    // a trace with no side trace wired runs none (and its dispatch leaves
    // the noting word as it was): none is wired while a trace runs
    if !ct.has_any_side_wired.get() || !ct.has_side_cells() {
        return None;
    }
    let noted = reg_state[trace_ran_side_slot(ct.window_size as usize)];
    let exit = ct.per_exit_inline.len() + usize::try_from(noted).ok()?.checked_sub(1)?;
    let child = ct.side_children.borrow().get(exit).cloned().flatten();
    debug_assert!(
        child.is_some(),
        "exit {exit} ran a side trace it does not hold"
    );
    Some(ExitSource {
        child,
        body: raw_ret,
        ran: true,
        off: 0,
        exit,
    })
}
