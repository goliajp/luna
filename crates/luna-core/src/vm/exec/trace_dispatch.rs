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
        let entry_fn = self.trace_entry_counted(&ct, cl.proto.call_hot_count.get());
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
        entry_tags.resize(max_stack, 0);
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
        // marshal-in fills the frame's registers; the rest start at zero
        reg_state.resize(reg_state_len, 0i64);
        reg_state[max_stack.min(reg_state_len)..].fill(0);
        let dispatch_ok = self.trace_marshal_in(
            base_us,
            max_stack,
            compile_entry_tags,
            &mut entry_tags,
            &mut reg_state,
            !ct.has_any_side_wired.get(),
        );

        if dispatch_ok {
            debug_assert_eq!(head_pc_val, pc, "trace cache hit's head_pc != pc");
            // A recording in progress cannot see what the trace runs
            // natively: it would resume after the trace with ops
            // missing, and could close as a loop that never ran (a
            // side trace of two ops returning its own head, which
            // the dispatcher then entered forever). Drop it.
            // Counted as a failure of that head: the per-head hot count
            // would otherwise start the same doomed recording again and
            // again.
            if let Some(rec) = self.jit.active_trace.take() {
                self.jit.counters.aborted += 1;
                self.jit.counters.bump_close_cause("reached-compiled-trace");
                note_trace_compile_failure(rec.head_proto, rec.head_pc);
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
                // SAFETY: `entry_fn` is the entry of `ct`, a compiled trace `cl.proto.traces` keeps alive; `reg_state` was sized above to the trace's window (plus the saved-pc slot for a down-recursion entry) and filled by `trace_marshal_in`; the guard above pins this Vm and `cl` for the helpers the trace calls
                unsafe { entry_fn(reg_state.as_mut_ptr()) }
            };
            self.jit.counters.dispatched += 1;
            if ct.inline_kinds != 0 {
                self.count_inline_kinds(ct.inline_kinds);
            }

            if self.jit.pending_err.is_some() {
                self.jit.pending_err = None;
                self.jit.counters.deopt += 1;
                // Unwind any helper-pushed
                // inlined frames before the interpreter resumes.
                // Don't restore reg_state — the trace's partial
                // writes are discarded; interp re-executes from
                // the original `pc`.
                while self.frames.len() > pre_frames {
                    self.pop_frame();
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
    /// `checked_only`: copy only the registers the trace checks. The others
    /// are held on the stack (see the lowering's `StackHeld`), and only a
    /// side trace run from one of its exits reads them, or their entry tags,
    /// from the buffers.
    fn trace_marshal_in(
        &self,
        base_us: usize,
        max_stack: usize,
        compile_entry_tags: &[u8],
        entry_tags: &mut [u8],
        reg_state: &mut [i64],
        checked_only: bool,
    ) -> bool {
        // one loop per value of `checked_only`: tested once per slot, it
        // cost two instructions on every register of every entry
        if checked_only {
            self.marshal_in_slots::<true>(
                base_us,
                max_stack,
                compile_entry_tags,
                entry_tags,
                reg_state,
            )
        } else {
            self.marshal_in_slots::<false>(
                base_us,
                max_stack,
                compile_entry_tags,
                entry_tags,
                reg_state,
            )
        }
    }

    #[inline(always)]
    fn marshal_in_slots<const CHECKED_ONLY: bool>(
        &self,
        base_us: usize,
        max_stack: usize,
        compile_entry_tags: &[u8],
        entry_tags: &mut [u8],
        reg_state: &mut [i64],
    ) -> bool {
        use crate::jit::trace::ENTRY_TAG_ANY;
        let frame = &self.stack[base_us..base_us + max_stack];
        let regs = &mut reg_state[..max_stack];
        let tags = &mut entry_tags[..max_stack];
        for i in 0..max_stack {
            if CHECKED_ONLY && compile_entry_tags.get(i) == Some(&ENTRY_TAG_ANY) {
                continue;
            }
            let (tag, payload) = frame[i].unpack();
            // SAFETY: every field of `RawVal` is one plain 8-byte word, and
            // `unpack` returned it fully initialised
            regs[i] = unsafe { payload.zero as i64 };
            // a slot past the compile-time tags is checked like one read
            let want = compile_entry_tags.get(i).copied().unwrap_or(tag);
            if want == ENTRY_TAG_ANY {
                // not read before the trace writes it: any value enters,
                // and an exit that has not written it leaves it as it is
                tags[i] = ENTRY_TAG_ANY;
                continue;
            }
            // The trace's IR is specialised to the compile-time entry
            // tags: on another, body ops would misread the raw bits (a Str
            // pointer as an Int payload). The interpreter runs this entry;
            // the trace stays for later ones. The payload of anything else
            // cannot stand for the value. A boolean, the one entry type
            // two tags enter, is left to the out-of-line test, so that the
            // common case costs one comparison and one bit test
            if tag != want || crate::jit::trace::PLAIN_ENTRY_TAGS >> tag & 1 == 0 {
                match bool_entry(want, tag) {
                    Some(p) => regs[i] = p,
                    None => return false,
                }
            }
            tags[i] = tag;
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
            self.pop_frame();
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

impl Vm {
    #[cold]
    fn count_inline_kinds(&mut self, kinds: u8) {
        for (b, n) in self
            .jit
            .counters
            .inline_kind_dispatched
            .iter_mut()
            .enumerate()
        {
            if kinds & (1 << b) != 0 {
                *n += 1;
            }
        }
    }

    /// The entry to call for `ct`, counting the entry towards its move to
    /// the optimizing tier and making that move when it is due. `calls`:
    /// the head function's `call_hot_count` now.
    fn trace_entry_counted(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
        calls: u32,
    ) -> crate::jit::trace::TraceFn {
        self.count_towards_tier_up(ct, calls);
        ct.current_entry()
    }

    /// Counts an entry of `ct` towards its move to the optimizing tier and
    /// makes the move when it is due. `calls`: the head function's
    /// `call_hot_count` now.
    pub(super) fn count_towards_tier_up(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
        calls: u32,
    ) {
        if let Some(t) = &ct.tier_up
            && !t.tried.get()
        {
            let n = t.count.get().wrapping_add(1);
            t.count.set(n);
            let reused =
                calls != t.calls_at && n >= t.at / crate::jit::trace::TIER_UP_REUSED_DIVISOR;
            if n >= t.at || reused {
                self.trace_tier_up(ct);
            }
        }
    }

    /// Hands a hot trace to the optimizing tier and points everything that
    /// enters it at the new code.
    #[cold]
    fn trace_tier_up(&mut self, ct: &crate::jit::trace::CompiledTrace) {
        let Some(t) = &ct.tier_up else { return };
        t.tried.set(true);
        let entry = {
            let jit = &mut self.jit;
            let storage: &mut dyn crate::jit::JitStorage = jit.storage.as_mut();
            jit.trace_compiler.tier_up(storage, ct)
        };
        // a backend still compiling better code keeps something in
        // `source`: ask again at the next entry when the trace runs the
        // optimizing tier's code, which does not count iterations, else once
        // the baseline code's count reaches `at` anew
        if t.source.borrow().is_some() {
            let counting = entry.is_none() && t.optimized.get().is_null();
            t.count
                .set(if counting { 0 } else { t.at.saturating_sub(1) });
            t.tried.set(false);
        }
        let Some(entry) = entry else { return };
        let p = entry as *const () as *const u8;
        t.optimized.set(p);
        for cell in &t.parent_cells {
            let c = cell.get() as *const crate::jit::send_compat::TCellPtr;
            if !c.is_null() {
                // SAFETY: the cell belongs to the parent trace, which stays
                // in its proto's `traces` as long as this child can run
                unsafe { (*c).set(p) };
            }
        }
        self.jit.counters.tiered_up += 1;
    }
}

/// The payload a register of tag `tag` enters a trace with whose entry tag
/// for it is `want`, when `tag` is not the plain match of `want`: a trace
/// compiled for a boolean takes either value, as payload 0 or 1. `None`:
/// the register cannot enter.
#[cold]
#[inline(never)]
fn bool_entry(want: u8, tag: u8) -> Option<i64> {
    use crate::runtime::value::raw;
    (want == raw::FALSE && (tag == raw::FALSE || tag == raw::TRUE)).then(|| i64::from(tag - want))
}
