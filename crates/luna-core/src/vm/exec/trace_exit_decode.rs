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
        if !ct.has_any_side_wired.get() {
            return parent_only;
        }
        // The side trace wired to the exit taken runs here rather than
        // from the parent's code: a test at every exit of the parent cost
        // more than it saved.
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

    /// Write the trace's registers back to the frame with the tags its exit
    /// analysis gives them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_restore_slots(
        &mut self,
        base_us: usize,
        max_stack: usize,
        keep_tfor: std::ops::Range<usize>,
        using_global_exit_tags: bool,
        global_tag_res_kind: crate::jit::trace::TagResKind,
        exit_tags_for_pc: &[crate::jit::trace::ExitTag],
        reg_state: &[i64],
        entry_tags: &[u8],
        child_ran: bool,
    ) {
        // At an inline cmp@d>0
        // side-exit, the helper has pushed N frames on
        // top of the trace head's frame and
        // `exit_tags_for_pc.len()` covers the full
        // window (caller + each inlined frame's
        // window). Slots beyond `max_stack` belong to
        // an inlined frame: their `Untouched` entries
        // default to Nil (no entry-tag fallback —
        // marshal-in only captured caller slots) and
        // we write to interp stack at `base + i` which
        // mirrors `op_offsets`-derived layout.
        let slot_count = exit_tags_for_pc.len();
        // The helper only extends
        // vm.stack up to the deepest pushed frame's
        // window, but the exit_tags snapshot covers
        // the trace's full `window_size` (which
        // includes depth-N+1 scratch slots that the
        // trace's IR may have written without a
        // matching pushed frame). Extend with Nil so
        // the write at the tail doesn't panic; these
        // slots get overwritten by the writeback loop
        // and won't leak meaningful data past the
        // pushed frames' R[0..max_stack) windows.
        if self.stack.len() < base_us + slot_count {
            self.stack
                .resize_or_abort(base_us + slot_count, crate::runtime::Value::Nil);
        }
        // Fast-path restore loop. When
        // we landed on the global `exit_tags`,
        // dispatch on the compile-time
        // classification: skip the loop entirely
        // for `AllUntouched`, do a tag-free
        // `Value::Int(...)` write per slot for
        // `AllInt`, otherwise fall through to the
        // general match-arm loop. site_id > 0
        // (inline frame mat) and per_exit_tags
        // hits always take the general path —
        // their per-side-exit shapes aren't
        // pre-classified yet.
        let fast_path_taken = if using_global_exit_tags && keep_tfor.is_empty() {
            match global_tag_res_kind {
                crate::jit::trace::TagResKind::AllUntouched => {
                    // No-op: vm.stack already
                    // matches the trace's post-
                    // entry state for these
                    // slots (entry values not
                    // overridden, or already
                    // spilled by helpers).
                    true
                }
                crate::jit::trace::TagResKind::AllInt => {
                    for i in 0..slot_count {
                        self.stack[base_us + i] = crate::runtime::Value::Int(reg_state[i]);
                    }
                    true
                }
                crate::jit::trace::TagResKind::Mixed => false,
            }
        } else {
            false
        };
        if !fast_path_taken {
            use crate::jit::trace::ExitTag;
            use crate::runtime::value::raw;
            // the raw tag each exit tag writes, by discriminant (a table
            // load where a `match` per slot was an indirect branch);
            // `Untouched` has none
            const UNTOUCHED: u8 = u8::MAX;
            // a boolean's tag is FALSE plus its payload
            const BOOL: u8 = u8::MAX - 1;
            const RAW_OF: [u8; 8] = {
                let mut m = [0; 8];
                m[ExitTag::Untouched as usize] = UNTOUCHED;
                m[ExitTag::Int as usize] = raw::INT;
                m[ExitTag::Float as usize] = raw::FLOAT;
                m[ExitTag::Table as usize] = raw::TABLE;
                m[ExitTag::Closure as usize] = raw::CLOSURE;
                // written nil (LoadNil): a nil whatever the entry tag
                m[ExitTag::Nil as usize] = raw::NIL;
                m[ExitTag::Str as usize] = raw::STR;
                m[ExitTag::Bool as usize] = BOOL;
                m
            };
            let frame = &mut self.stack[base_us..base_us + slot_count];
            let regs = &reg_state[..slot_count];
            for (i, &exit_tag) in exit_tags_for_pc.iter().enumerate() {
                let mut tag = RAW_OF[exit_tag as usize];
                // `Untouched` first: a trace leaves most slots unwritten,
                // and a written boolean is rare
                if tag == UNTOUCHED {
                    if i >= max_stack {
                        tag = raw::NIL;
                    } else {
                        // not written: the stack holds the value it entered
                        // with, unless a side trace ran after the trace that
                        // wrote it
                        if !child_ran {
                            continue;
                        }
                        tag = entry_tags[i];
                        // not checked on entry and not written since: the
                        // stack still holds the value
                        if tag == crate::jit::trace::ENTRY_TAG_ANY {
                            continue;
                        }
                    }
                } else if tag == BOOL {
                    tag = raw::FALSE + (regs[i] & 1) as u8;
                }
                if keep_tfor.contains(&i) {
                    continue;
                }
                // SAFETY: the tag is the slot's entry tag (checked on
                // entry) or the kind the exit analysis pins its payload
                // to; the payload sits in reg_state[i].
                unsafe {
                    Value::pack_into(
                        &mut frame[i],
                        tag,
                        crate::runtime::value::RawVal {
                            zero: regs[i] as u64,
                        },
                    )
                };
            }
        }
    }
}

/// Which trace's exit decodes a trace's return.
pub(super) struct ExitSource {
    /// The side trace whose exit shapes decode the return (`None`: the
    /// trace's own).
    pub(super) child: Option<TArc<CompiledTrace>>,
    /// The return bits to decode.
    pub(super) body: u64,
    /// A side trace ran.
    pub(super) ran: bool,
    /// The register of the parent's window the side trace's register 0 is.
    pub(super) off: usize,
}
