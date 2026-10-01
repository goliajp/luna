//! The trace JIT dispatcher: entering a compiled trace at the pc the
//! interpreter is about to run, and resuming the interpreter where it exits.

use super::*;

impl Vm {
    /// Run the trace cached for `pc` of `cl` (frame `base`), if one admits this
    /// entry. `true` when the trace ran and the frame's pc now points past it.
    #[inline(never)]
    pub(super) fn trace_dispatch(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        base: u32,
        downrec_admit_blocked: bool,
    ) -> bool {
        let Some(ct) = ({
            let traces = cl.proto.traces.borrow();
            traces
                .iter()
                .find(|t| {
                    if t.head_pc != pc {
                        return false;
                    }
                    let is_downrec = t.downrec_link.is_some();
                    // The one-shot suppress
                    // flag blocks any admit (primary or fallback)
                    // for `downrec_link`-bearing traces so the
                    // next interp iter can run the natural op
                    // at `head_pc` and advance past it. The multi-way
                    // `dispatchable=true` lift means the suppress
                    // must also cover the primary `t.dispatchable`
                    // arm — otherwise the lifted lookup would
                    // immediately re-admit after a force-deopt
                    // and the infinite loop returns.
                    if downrec_admit_blocked {
                        return false;
                    }
                    // Primary arm: `dispatchable=true` traces
                    // (lifted multi-way DownRec or normal traces).
                    // Fallback arm: single-CMP `dispatchable=false`
                    // DownRec traces (single-CMP guard kept
                    // pinned because the 90% miss-rate would
                    // make blind admit perf-negative).
                    t.dispatchable || is_downrec
                })
                .cloned()
        }) else {
            return false;
        };
        // Borrow Rc<[T]> fields as &Rc<[T]> instead
        // of cloning. The outer `ct: Rc<CompiledTrace>` is held
        // across the entire dispatch block so the fields outlive
        // all consumers.
        let entry_fn = ct.entry;
        let head_pc_val = ct.head_pc;
        let window_size = ct.window_size;
        let compile_entry_tags = &ct.entry_tags;
        let max_stack = cl.proto.max_stack as usize;
        let window_size_us = window_size as usize;
        let base_us = base as usize;
        // `reg_state` sized to the trace's `window_size`, which
        // may exceed max_stack.
        // Marshal-in still only writes [0..max_stack); slots
        // [max_stack..window_size) are zero-initialised and
        // filled by the trace's own GetUpval / arith.
        // Reuse the Vm's amortised buffers
        // instead of allocating fresh Vecs each dispatch.
        // mem::take leaves an empty placeholder we restore
        // at the end of the dispatch block (success +
        // deopt paths both fall through to the restore).
        let mut entry_tags: Vec<u8> = std::mem::take(&mut self.jit.entry_tags_buf);
        entry_tags.clear();
        entry_tags.reserve(max_stack);
        // This trace was admitted via the
        // `downrec_link.is_some()` arm rather than the normal
        // `dispatchable=true` arm. The pre-invoke path
        // populates a reserved saved-PC slot just past the
        // normal register window so the lowerer's guard load
        // (`reg_state[window_size]`) compares the runtime
        // saved caller PC against the recorded `dr_return_pc`.
        //
        // No `!ct.dispatchable` gate: when the lowerer lifts
        // `dispatchable = true` for multi-way guards, the
        // trace's body still emits the downrec sentinel shape
        // on return — the saved-PC slot
        // and post-invoke classifier must keep firing.
        // `downrec_link.is_some()` is the unique structural
        // signal that the trace closes via DownRec.
        let is_downrec_entry = ct.downrec_link.is_some();
        let mut reg_state: Vec<i64> = std::mem::take(&mut self.jit.reg_state_buf);
        reg_state.clear();
        // When admitting a downrec trace,
        // size the buffer to `window_size + 1` so the lowerer
        // can `load(I64, ..., reg_state, window_size * 8)`
        // for the saved caller PC guard input. The extra slot
        // is the LAST element so cranelift's existing
        // `0..window_size` accesses are unaffected.
        let reg_state_len = if is_downrec_entry {
            window_size_us + 1
        } else {
            window_size_us
        };
        reg_state.resize(reg_state_len, 0i64);
        let dispatch_ok = self.trace_marshal_in(
            base_us,
            max_stack,
            compile_entry_tags,
            &mut entry_tags,
            &mut reg_state,
        );

        if dispatch_ok {
            debug_assert_eq!(head_pc_val, pc, "trace cache hit's head_pc != pc");
            // A recording in progress cannot see what the trace runs
            // natively: it would resume after the trace with ops
            // missing, and could close as a loop that never ran (a
            // side trace of two ops returning its own head, which
            // the dispatcher then entered forever). Drop it.
            if self.jit.active_trace.take().is_some() {
                self.jit.counters.aborted += 1;
                self.jit.counters.bump_close_cause("reached-compiled-trace");
            }
            self.jit.pending_err = None;
            // Snapshot the pre-entry frame
            // count. A cmp@d>0 side-exit calls the materialize
            // helper which pushes inlined frames onto
            // `vm.frames`; on deopt those frames must be popped
            // before falling through to the interpreter, else
            // the stack grows unboundedly per deopted dispatch.
            let pre_frames = self.frames.len();
            if is_downrec_entry {
                reg_state[window_size_us] = self.downrec_saved_pc(pre_frames);
            }
            // `LUNA_AOT_PROBE`
            // diagnostic hook. The probe fires once per trace dispatch
            // (regardless of JIT vs AOT origin — both go through this
            // arm), letting the AOT smoke test verify mcode actually
            // executed. Guarded behind `OnceLock` so the env read is
            // a one-time cost per process; not gated on a particular
            // counter so the smoke test gets a deterministic single-
            // line `aot_trace_fired pc=N` per first dispatch.
            if jit_probe_enabled() && self.jit.counters.dispatched == 0 {
                eprintln!("luna-runtime-helpers: aot_trace_fired pc={head_pc_val}");
            }
            let continuation_pc = {
                // chunk_compiler.enter
                // (CraneliftBackend delegates to enter_jit;
                // NullJitBackend returns an inert guard).
                let vm_ptr: *mut Vm = self;
                let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { entry_fn(reg_state.as_mut_ptr()) }
            };
            self.jit.counters.dispatched += 1;

            if self.jit.pending_err.is_some() {
                self.jit.pending_err = None;
                self.jit.counters.deopt += 1;
                // Unwind any helper-pushed
                // inlined frames before the interpreter resumes.
                // Don't restore reg_state — the trace's partial
                // writes are discarded; interp re-executes from
                // the original `pc`.
                while self.frames.len() > pre_frames {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                }
                if is_downrec_entry {
                    // pending_err observed
                    // mid-trace inside a downrec admit. Treat
                    // it as a guard miss: bump `downrec_deopt`
                    // and suppress the next downrec admit so
                    // interp can advance past `head_pc` and
                    // the same trace doesn't immediately re-
                    // fire on the next loop iteration.
                    self.jit.counters.downrec_deopt += 1;
                    self.jit.suppress_downrec_admit_once = true;
                }
            } else if is_downrec_entry && downrec_close_exit(continuation_pc, head_pc_val) {
                self.trace_downrec_classify(continuation_pc, pre_frames);
                self.jit.reg_state_buf = reg_state;
                self.jit.entry_tags_buf = entry_tags;
                return true;
            } else {
                self.trace_exit_restore(
                    cl,
                    pc,
                    base,
                    &ct,
                    continuation_pc,
                    pre_frames,
                    reg_state,
                    entry_tags,
                );
                return true;
            }
        }
        // !dispatch_ok / deopt path / non-cont
        // exit also restore the buffers before falling
        // through to the interp.
        self.jit.reg_state_buf = reg_state;
        self.jit.entry_tags_buf = entry_tags;
        false
    }

    /// Copy the frame's registers into the trace's entry buffer. `false` when a
    /// register the trace checks has a tag other than the one the trace was
    /// compiled for, or one that cannot be passed as a raw payload.
    fn trace_marshal_in(
        &self,
        base_us: usize,
        max_stack: usize,
        compile_entry_tags: &[u8],
        entry_tags: &mut Vec<u8>,
        reg_state: &mut [i64],
    ) -> bool {
        for i in 0..max_stack {
            let v = self.stack[base_us + i];
            let (tag, raw) = v.unpack();
            let want = compile_entry_tags.get(i).copied();
            if want == Some(crate::jit::trace::ENTRY_TAG_ANY) {
                // not read before the trace writes it: any value enters,
                // and an exit that has not written it leaves it as it is
                // (the restore skips a slot whose entry tag is ANY)
                entry_tags.push(crate::jit::trace::ENTRY_TAG_ANY);
                // SAFETY: the raw payload of the slot's own value.
                reg_state[i] = unsafe { raw.zero as i64 };
                continue;
            }
            entry_tags.push(tag);
            // Entry tag guard. The trace's IR
            // is specialised to the compile-time entry tags
            // (via current_kinds propagation from
            // from_entry_tag). A runtime tag mismatch means
            // body ops would mis-interpret raw bits (e.g.
            // treat a Str pointer as Int payload → garbage).
            // Skip dispatch on mismatch so interp handles
            // this entry shape; the trace stays cached for
            // future entries that match.
            if want.is_some_and(|w| tag != w) {
                return false;
            }
            match tag {
                // Int / Float / Table / Nil all marshal
                // to raw payload cleanly; the trace's IR
                // treats the 8-byte slot as an i64 (with
                // f64 ops bitcasting around the boundary).
                crate::runtime::value::raw::INT
                | crate::runtime::value::raw::FLOAT
                | crate::runtime::value::raw::TABLE
                | crate::runtime::value::raw::CLOSURE
                // Native iter slots (e.g.
                // R[A] = ipairs_iter) are present in
                // generic-for traces; the raw bits are a
                // valid `*mut NativeClosure` and round-trip
                // cleanly.
                | crate::runtime::value::raw::NATIVE
                // Str slots show up in
                // string-concat traces; raw bits = `*mut
                // LuaStr` (interned, GC-managed). Round-
                // trips cleanly as a heap pointer.
                | crate::runtime::value::raw::STR
                | crate::runtime::value::raw::NIL => {
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                    reg_state[i] = unsafe { raw.zero as i64 };
                }
                _ => {
                    return false;
                }
            }
        }
        true
    }

    /// The downrec trace returned through its close: count the stitch hit or
    /// miss, block the next downrec admit, and pop frames the trace pushed.
    fn trace_downrec_classify(&mut self, continuation_pc: i64, pre_frames: usize) {
        // Downrec event classifier.
        let raw_ret = continuation_pc as u64;
        let sentinel_code = ((raw_ret >> 56) & 0x7F) as u32;
        if crate::jit::trace_types::is_downrec_sentinel(sentinel_code) {
            // Guard HIT — saved_pc matched one of the
            // baked candidates and the trace's
            // `stitch_blk` arm returned the DOWNREC
            // sentinel. Cycle-safety checkpoint:
            // decrement budget; on underflow,
            // reclassify as deopt + reset budget.
            // `STITCH_DEPTH_DEFAULT = 32` lets
            // ~all natural HITs in a hot loop fire
            // before reset pressure.
            if self.jit.stitch_depth_remaining > 0 {
                self.jit.stitch_depth_remaining -= 1;
                self.jit.counters.downrec_dispatched += 1;
            } else {
                self.jit.counters.downrec_deopt += 1;
                self.jit.stitch_depth_remaining =
                    crate::vm::jit_state::JitState::STITCH_DEPTH_DEFAULT;
            }
        } else {
            // Guard MISS via the lowerer's deopt_blk
            // arm (GLOBAL sentinel + body == head_pc).
            // The deopt_blk emit performs the
            // store-back via `emit_store_back_and_return_pc`,
            // so the live stack already reflects the
            // body's writes; no extra restore needed
            // from the dispatcher side.
            self.jit.counters.downrec_deopt += 1;
        }
        self.jit.suppress_downrec_admit_once = true;
        // Pop helper-pushed inlined frames (defensive —
        // the downrec emit shape doesn't push frames in the
        // tail, but a body side-exit before reaching
        // the tail may have via the materialize helper).
        while self.frames.len() > pre_frames {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        }
    }

    /// The caller's pc a downrec trace guards on.
    // Saved-PC slot population. The
    // recorded `dr_return_pc` on the closing trace is
    // the caller's resume PC captured at a depth>0
    // Return push (recorder push site). The natural runtime analogue for self-
    // stitch is the dispatching frame's PARENT frame's
    // PC: the trace's head_pc sits inside a Lua frame,
    // and the parent (caller) frame's `pc` is what
    // luna would observe as `[base-8]` in the LJ
    // `asm_retf` shape (`lj_asm_arm64.h:565`). When
    // the parent isn't a Lua frame (top-level dispatch
    // — first invocation through `call_value`), no
    // saved PC exists; we write 0, which always
    // mismatches the recorded `dr_return_pc != 0`
    // invariant (debug-asserted in the luna-jit trace
    // lowerer).
    fn downrec_saved_pc(&self, pre_frames: usize) -> i64 {
        if pre_frames >= 2 {
            match &self.frames[pre_frames - 2] {
                CallFrame::Lua(parent) => parent.pc as i64,
                CallFrame::Cont(_) => 0,
            }
        } else {
            0
        }
    }
}

/// Whether a downrec trace's return is its close: the stitch_blk DOWNREC
/// sentinel (hit) or the deopt_blk GLOBAL sentinel with body == head_pc (miss).
fn downrec_close_exit(continuation_pc: i64, head_pc_val: u32) -> bool {
    // Only enter the
    // downrec classifier for returns whose shape
    // matches the lowerer's `downrec_idx_opt` tail
    // emit: either the stitch_blk DOWNREC sentinel
    // (HIT) or the deopt_blk GLOBAL-sentinel-with-
    // body==head_pc (MISS via guard fail). Any
    // other return from a downrec trace (intermediate
    // body cmp side-exit, GetField inference fail,
    // etc.) carries a different sentinel/body shape
    // and means the body exited BEFORE reaching the
    // downrec close — classify those through the
    // normal decode path (else branch below) so
    // reg_state restores + pc advances correctly.
    // Classifying them all as MISS would skip the
    // normal restore, inflating `downrec_deopt` with
    // non-downrec events and losing the trace's
    // mid-flight writes.
    let raw_ret = continuation_pc as u64;
    let from_side_trace = (raw_ret >> 63) & 1 == 1;
    let sentinel_code = if from_side_trace {
        ((raw_ret >> 56) & 0x7F) as u32
    } else {
        0
    };
    let raw_body = raw_ret & 0x00FF_FFFF_FFFF_FFFFu64;
    let global_deopt_code = crate::jit::trace_types::encode_side_sentinel(
        crate::jit::trace_types::SIDE_SENT_KIND_GLOBAL,
        0,
    );
    from_side_trace
        && (crate::jit::trace_types::is_downrec_sentinel(sentinel_code)
            || (sentinel_code == global_deopt_code && raw_body == head_pc_val as u64))
}
