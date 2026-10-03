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
        pc: u32,
        ct: &CompiledTrace,
        raw_ret: u64,
        reg_state: &mut [i64],
        base_us: usize,
        entry_tags: &[u8],
    ) -> ExitSource {
        let src = self.exit_source_of(cl, pc, ct, raw_ret, reg_state, base_us, entry_tags);
        // a side trace runs one pass for each return through here, and the
        // dispatcher never enters it: count the pass as its entry
        if src.2
            && let Some(c) = &src.0
        {
            self.count_towards_tier_up(c, cl.proto.call_hot_count.get());
        }
        src
    }

    fn exit_source_of(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        ct: &CompiledTrace,
        raw_ret: u64,
        reg_state: &mut [i64],
        base_us: usize,
        entry_tags: &[u8],
    ) -> ExitSource {
        let window_size = ct.window_size;
        let from_side_trace = (raw_ret >> 63) & 1 == 1;
        if from_side_trace {
            let sentinel_code = ((raw_ret >> 56) & 0x7F) as u32;
            let body = raw_ret & 0x00FF_FFFF_FFFF_FFFFu64;
            let traces = cl.proto.traces.borrow();
            let child_idx = ct.side_trace_cache.borrow().get(&sentinel_code).copied();
            if let Some(idx) = child_idx
                && let Some(child) = traces.get(idx as usize)
            {
                if crate::jit::trace::v2c_probe_enabled() {
                    eprintln!(
                        "[v2c-A3-decode] sentinel={:#04x} body={:#018x} child_idx={} child.n_ops={} child.head_pc={} child.window_size={} parent.pc={} parent.window_size={} child.dispatchable={} child.inline_abort={}",
                        sentinel_code,
                        body,
                        idx,
                        child.n_ops,
                        child.head_pc,
                        child.window_size,
                        pc,
                        window_size,
                        child.dispatchable,
                        child.is_inline_abort_close,
                    );
                }
                (Some(child.clone()), body, true)
            } else {
                if crate::jit::trace::v2c_probe_enabled() {
                    eprintln!(
                        "[v2c-A3-decode] sentinel={:#04x} body={:#018x} child MISS (fallback parent shapes)",
                        sentinel_code, body,
                    );
                }
                // Cache miss — fall back to parent
                // shapes with the body bits. Best-
                // effort; the trace_side_trace_
                // shape_mismatch_count records this
                // path indirectly (close-handler
                // skips wiring on mismatch so we
                // shouldn't reach here when shape
                // gate held).
                (None, body, true)
            }
        } else if !ct.has_any_side_wired.get() {
            (None, raw_ret, false)
        } else {
            // Dispatcher-level side-trace invocation,
            // rather than an IR gate (`load + icmp +
            // brif`) at every emit_store_back callsite,
            // which measured as a net slowdown.
            let tentative = crate::jit::trace::decode_exit_shape(
                raw_ret,
                &ct.per_exit_inline,
                &ct.per_exit_tags,
                &ct.exit_tags,
            );
            let fn_ptr = ct
                .exit_side_trace_ptrs
                .get(tentative.exit_hit_idx)
                .map_or(std::ptr::null(), |cell| cell.get());
            let child = (!fn_ptr.is_null())
                .then(|| {
                    cl.proto
                        .traces
                        .borrow()
                        .iter()
                        .find(|t| t.current_entry() as *const () as *const u8 == fn_ptr)
                        .cloned()
                })
                .flatten();
            if let Some(child) = child
                && self.child_reads_stack_held_ok(base_us, entry_tags, &child.entry_tags)
            {
                let cent = child.current_entry();
                let child_raw_ret = {
                    // chunk_compiler.enter
                    // (side-trace entry).
                    let vm_ptr: *mut Vm = self;
                    let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                    // SAFETY: `cent` is the entry of a side trace compiled for this exit and found in `cl.proto.traces`, which keeps its code alive; `reg_state` is the register window the parent ran on, whose tags were just checked against what the child reads; the guard above pins this Vm and `cl` for the helpers the trace calls
                    unsafe { cent(reg_state.as_mut_ptr()) }
                };
                (Some(child), child_raw_ret as u64, true)
            } else {
                (None, raw_ret, false)
            }
        }
    }

    /// Whether a side trace may run on the registers as they are: a slot
    /// the parent took unchecked (its runtime entry tag is ANY) but the
    /// child reads holds, on the stack, a value of the tag the child was
    /// compiled for.
    fn child_reads_stack_held_ok(
        &self,
        base_us: usize,
        entry_tags: &[u8],
        child_entry: &[u8],
    ) -> bool {
        entry_tags
            .iter()
            .zip(child_entry)
            .enumerate()
            .all(|(i, (&p, &c))| {
                p != crate::jit::trace::ENTRY_TAG_ANY
                    || c == crate::jit::trace::ENTRY_TAG_ANY
                    || self.stack[base_us + i].unpack().0 == c
            })
    }

    /// Write the trace's registers back to the frame with the tags its exit
    /// analysis gives them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_restore_slots(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        max_stack: usize,
        decode_body: u64,
        cont_pc: u32,
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
                .resize(base_us + slot_count, crate::runtime::Value::Nil);
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
        // A generic-for exit whose TForCall wrote the loop
        // variables to the stack with tags the trace did not
        // compile for: leave those slots as they are.
        let keep_tfor = if decode_body & crate::jit::trace_types::EXIT_KEEP_TFOR_VARS != 0 {
            let call = cl.proto.code[cont_pc as usize - 1];
            debug_assert!(matches!(call.op(), crate::vm::isa::Op::TForCall));
            let first = call.a() as usize + 4;
            first..first + call.c() as usize
        } else {
            0..0
        };
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
                if tag == BOOL {
                    tag = raw::FALSE + (regs[i] & 1) as u8;
                } else if tag == UNTOUCHED {
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

/// The side trace whose exit shapes decode a trace's return (`None`: the
/// trace's own), the return bits themselves, and whether a side trace ran.
pub(super) type ExitSource = (Option<TArc<CompiledTrace>>, u64, bool);
