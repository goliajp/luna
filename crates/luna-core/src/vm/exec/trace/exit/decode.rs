//! Decoding a trace's return: which trace's exit shapes apply (a side
//! trace may have run) and the register write-back with the exit's tags.

use super::*;
use crate::jit::trace::CompiledTrace;
use crate::jit::trace_types::frame::TRACE_FRAME_TAIL;

impl Vm {
    /// Which trace's exit shapes decode `raw_ret`: a side trace's when one
    /// is wired to the exit taken (it runs here first), else the trace's
    /// own.
    pub(in crate::vm::exec) fn trace_exit_source(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: &CompiledTrace,
        raw_ret: u64,
        reg_state: &mut Vec<i64>,
        base_us: usize,
        entry_tags: &[u8],
    ) -> ExitSource {
        let src = self.exit_source_of(cl, ct, raw_ret, reg_state, base_us, entry_tags);
        // a side trace runs one pass for each return through here, and the
        // dispatcher never enters it: count the pass as its entry
        if src.ran
            && let Some(c) = &src.child
        {
            self.count_towards_tier_up(c, cl.proto.call_hot_count.get());
        }
        src
    }

    fn exit_source_of(
        &mut self,
        cl: Gc<LuaClosure>,
        ct: &CompiledTrace,
        raw_ret: u64,
        reg_state: &mut Vec<i64>,
        base_us: usize,
        entry_tags: &[u8],
    ) -> ExitSource {
        let parent_only = ExitSource {
            child: None,
            body: raw_ret,
            ran: false,
            off: 0,
        };
        // a collection-check exit runs no side trace: the check is the op's
        if !ct.has_any_side_wired.get() || self.jit.gc_exit.is_some() {
            return parent_only;
        }
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
            || !self.child_reads_stack_held_ok(base_us, off, entry_tags, &child.plans.entry_tags)
        {
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
                    if reg_state.len() < need {
                        reg_state.resize(need, 0);
                    }
                    if need > frame_end {
                        reg_state[frame_end..need].fill(0);
                    }
                    f.closure
                }
                _ => return parent_only,
            }
        };
        child_frame_in(reg_state, off, &child);
        let cent = child.current_entry();
        self.count_side_trace_run(off);
        let child_raw_ret = {
            let vm_ptr: *mut Vm = self;
            let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(child_cl));
            // SAFETY: `cent` is the entry of a side trace compiled for this exit and held by `ct`, whose code lives as long as this Vm; from register `off` on, `reg_state` holds at least the child's window and the three words past it, and the registers the child reads carry the tags it was compiled for (checked when it was wired, and the stack-held ones just above); `child_cl` runs the frame the child starts in, and the guard pins it and this Vm for the helpers the trace calls
            unsafe { cent(reg_state.as_mut_ptr().add(off)) }
        };
        ExitSource {
            child: Some(child),
            body: child_raw_ret as u64,
            ran: true,
            off,
        }
    }

    fn count_side_trace_run(&mut self, off: usize) {
        self.jit.counters.side_trace_runs += 1;
        if off > 0 {
            self.jit.counters.side_trace_runs_inlined += 1;
        }
    }

    /// Whether a side trace may run on the registers as they are: a slot
    /// the parent took unchecked (its runtime entry tag is ANY) but the
    /// child reads holds, on the stack, a value of the tag the child was
    /// compiled for. The child's register `i` is the parent's `off + i`.
    fn child_reads_stack_held_ok(
        &self,
        base_us: usize,
        off: usize,
        entry_tags: &[u8],
        child_entry: &[u8],
    ) -> bool {
        entry_tags
            .iter()
            .skip(off)
            .zip(child_entry)
            .enumerate()
            .all(|(i, (&p, &c))| {
                p != crate::jit::trace::ENTRY_TAG_ANY
                    || c == crate::jit::trace::ENTRY_TAG_ANY
                    || self.stack[base_us + off + i].unpack().0 == c
            })
    }
}

/// Sizes `reg_state` to hold `child`'s window and the words past it from
/// `reg_state[off]`, its register 0. A side trace never counts in its
/// call's words (`wire_side_trace` refuses one that does), so it reads none
/// of them, and the parent's registers they may lie on stay as they are.
fn child_frame_in(reg_state: &mut Vec<i64>, off: usize, child: &CompiledTrace) {
    debug_assert!(!child.call_count, "a side trace counting per call");
    let tail = off + child.window_size as usize + TRACE_FRAME_TAIL;
    if reg_state.len() < tail {
        reg_state.resize(tail, 0);
    }
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
}
