//! Decoding a trace's return: which trace's exit shapes apply (a side
//! trace may have run) and the register write-back with the exit's tags.

use super::*;
use crate::jit::trace::CompiledTrace;

impl Vm {
    /// Which trace's exit shapes decode `raw_ret`: a side trace's when the
    /// parent tail-called one (or one is wired to the exit taken, which runs
    /// here first), else the trace's own.
    pub(super) fn trace_exit_source(
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
        if (raw_ret >> 63) & 1 == 1 {
            // the parent's code ran the side trace wired to one of its
            // exits; the sentinel names the exit
            let sentinel_code = ((raw_ret >> 56) & 0x7F) as u32;
            let exit = ct.side_trace_cache.borrow().get(&sentinel_code).copied();
            let child = exit.and_then(|e| ct.side_children.borrow().get(&e).cloned());
            return ExitSource {
                child,
                body: raw_ret & 0x00FF_FFFF_FFFF_FFFFu64,
                ran: true,
                off: exit.map_or(0, |e| ct.exit_frame_offset(e as usize)),
            };
        }
        // a collection-check exit runs no side trace: the check is the op's
        if !ct.has_any_side_wired.get() || self.jit.gc_exit.is_some() {
            return parent_only;
        }
        // the side trace wired to the exit taken runs here, not from the
        // parent's code (a test at every exit cost more than it saved)
        let tentative = crate::jit::trace::decode_exit_shape(
            raw_ret,
            &ct.per_exit_inline,
            &ct.per_exit_tags,
            &ct.exit_tags,
        );
        let exit = tentative.exit_hit_idx;
        let Some(child) = ct.side_children.borrow().get(&(exit as u32)).cloned() else {
            return parent_only;
        };
        // a side trace was recorded from the pc its exit resumed at; a
        // plain-pc exit can return other pcs (a for loop's tail goes back
        // to the body or leaves the loop), and the child only continues
        // from its own
        let off = ct.exit_frame_offset(exit);
        if child.head_pc != tentative.cont_pc
            || !self.child_reads_stack_held_ok(base_us, off, entry_tags, &child.entry_tags)
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
        let cent = child.current_entry();
        self.jit.counters.side_trace_runs += 1;
        if off > 0 {
            self.jit.counters.side_trace_runs_inlined += 1;
        }
        let child_raw_ret = {
            let vm_ptr: *mut Vm = self;
            let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(child_cl));
            // SAFETY: `cent` is the entry of a side trace compiled for this exit and held by `ct`, whose code lives as long as this Vm; from register `off` on, `reg_state` holds at least the child's window, and the registers the child reads carry the tags it was compiled for (checked when it was wired, and the stack-held ones just above); `child_cl` runs the frame the child starts in, and the guard pins it and this Vm for the helpers the trace calls
            unsafe { cent(reg_state.as_mut_ptr().add(off)) }
        };
        ExitSource {
            child: Some(child),
            body: child_raw_ret as u64,
            ran: true,
            off,
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

/// Which trace's exit decodes a trace's return.
pub(super) struct ExitSource {
    /// The side trace whose exit shapes decode the return (`None`: its own).
    pub(super) child: Option<TArc<CompiledTrace>>,
    /// The return bits to decode.
    pub(super) body: u64,
    /// A side trace ran.
    pub(super) ran: bool,
    /// The register of the parent's window the side trace's register 0 is.
    pub(super) off: usize,
}
