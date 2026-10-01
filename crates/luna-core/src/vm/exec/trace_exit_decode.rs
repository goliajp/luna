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
        let head_pc_val = ct.head_pc;
        let window_size = ct.window_size;
        let exit_tags = &ct.exit_tags;
        let per_exit_tags = &ct.per_exit_tags;
        let per_exit_inline = &ct.per_exit_inline;
        let exit_hit_counts = &ct.exit_hit_counts;
        let from_side_trace = (raw_ret >> 63) & 1 == 1;
        if from_side_trace {
            let sentinel_code = ((raw_ret >> 56) & 0x7F) as u32;
            let body = raw_ret & 0x00FF_FFFF_FFFF_FFFFu64;
            let traces = cl.proto.traces.borrow();
            let child_idx = traces
                .iter()
                .find(|t| t.head_pc == head_pc_val)
                .and_then(|pct| pct.side_trace_cache.borrow().get(&sentinel_code).copied());
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
                (
                    child.per_exit_inline.clone(),
                    child.per_exit_tags.clone(),
                    child.exit_tags.clone(),
                    child.exit_hit_counts.clone(),
                    body,
                    true,
                )
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
                (
                    per_exit_inline.clone(),
                    per_exit_tags.clone(),
                    exit_tags.clone(),
                    exit_hit_counts.clone(),
                    body,
                    true,
                )
            }
        } else {
            // Dispatcher-level side-trace invocation,
            // rather than an IR gate (`load + icmp +
            // brif`) at every emit_store_back callsite,
            // which measured as a net slowdown. The
            // tentative decode + cell load always runs:
            // short-circuiting it on a
            // `parent_has_side` hint measured slower on
            // btrees_d8 and no faster on fib_10.
            {
                let tentative = crate::jit::trace::decode_exit_shape(
                    raw_ret,
                    per_exit_inline,
                    per_exit_tags,
                    exit_tags,
                );
                let tentative_exit_idx = tentative.exit_hit_idx;
                let child_invoke = {
                    let traces = cl.proto.traces.borrow();
                    traces
                        .iter()
                        .find(|t| t.head_pc == head_pc_val)
                        .and_then(|pct| {
                            let cell = pct.exit_side_trace_ptrs.get(tentative_exit_idx)?;
                            let fn_ptr = cell.get();
                            if fn_ptr.is_null() {
                                return None;
                            }
                            traces
                                .iter()
                                .find(|t| t.entry as *const () as *const u8 == fn_ptr)
                                .map(|child| {
                                    (
                                        child.entry,
                                        child.per_exit_inline.clone(),
                                        child.per_exit_tags.clone(),
                                        child.exit_tags.clone(),
                                        child.exit_hit_counts.clone(),
                                        child.entry_tags.clone(),
                                    )
                                })
                        })
                };
                if let Some((cent, cpi, cpt, cet, chc, cent_tags)) = child_invoke
                    && self.child_reads_stack_held_ok(base_us, entry_tags, &cent_tags)
                {
                    let child_raw_ret = {
                        // chunk_compiler.enter
                        // (side-trace entry).
                        let vm_ptr: *mut Vm = self;
                        let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        unsafe { cent(reg_state.as_mut_ptr()) }
                    };
                    (cpi, cpt, cet, chc, child_raw_ret as u64, true)
                } else {
                    (
                        per_exit_inline.clone(),
                        per_exit_tags.clone(),
                        exit_tags.clone(),
                        exit_hit_counts.clone(),
                        raw_ret,
                        false,
                    )
                }
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
            for i in 0..slot_count {
                if keep_tfor.contains(&i) {
                    continue;
                }
                let tag = match exit_tags_for_pc[i] {
                    crate::jit::trace::ExitTag::Untouched if i < max_stack => {
                        match entry_tags[i] {
                            // not checked on entry and not written since:
                            // the stack still holds the value
                            crate::jit::trace::ENTRY_TAG_ANY => continue,
                            t => t,
                        }
                    }
                    crate::jit::trace::ExitTag::Untouched => crate::runtime::value::raw::NIL,
                    crate::jit::trace::ExitTag::Int => crate::runtime::value::raw::INT,
                    crate::jit::trace::ExitTag::Float => crate::runtime::value::raw::FLOAT,
                    crate::jit::trace::ExitTag::Table => crate::runtime::value::raw::TABLE,
                    crate::jit::trace::ExitTag::Closure => crate::runtime::value::raw::CLOSURE,
                    // Trace actively wrote Nil
                    // to this slot (e.g. via Op::LoadNil).
                    // Restore as Nil regardless of the entry
                    // tag, since the i64 payload is 0 and
                    // packing as the entry tag (e.g. INT)
                    // would mis-type the slot.
                    crate::jit::trace::ExitTag::Nil => crate::runtime::value::raw::NIL,
                    // Trace wrote a Str ptr
                    // to this slot (LoadK Str / Move from
                    // Str / Concat result). Restore as
                    // Value::Str with raw bits round-
                    // tripped.
                    crate::jit::trace::ExitTag::Str => crate::runtime::value::raw::STR,
                };
                // SAFETY: tag is from a verified slot
                // (entry validated above) or pinned by
                // the exit-tag analysis to INT/TABLE.
                // The raw payload sits in reg_state[i].
                // Stack was extended by the materialize
                // helper for inline frames.
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                self.stack[base_us + i] = unsafe {
                    Value::pack(
                        tag,
                        crate::runtime::value::RawVal {
                            zero: reg_state[i] as u64,
                        },
                    )
                };
            }
        }
    }
}

/// Exit shapes to decode a trace's return with, the return bits themselves,
/// and whether a side trace ran.
pub(super) type ExitSource = (
    TArc<[crate::jit::trace_types::InlineSideExit]>,
    TArc<[(u32, TArc<[crate::jit::trace::ExitTag]>)]>,
    TArc<[crate::jit::trace::ExitTag]>,
    TArc<[crate::jit::send_compat::TCellU32]>,
    u64,
    bool,
);
