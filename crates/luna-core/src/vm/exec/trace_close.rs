//! Closing a trace recording: discard a call-triggered recording that covers
//! too little of its function, otherwise compile it, wire a side trace into
//! its parent's exit and cache it on the head proto.

use super::*;
use crate::jit::trace::{CompiledTrace, TraceRecord};

impl Vm {
    /// The active recording reached a close condition.
    pub(super) fn trace_close_recording(&mut self) {
        let rec = self.jit.active_trace.as_mut().expect("recording");
        // Long-trace bias: a call-triggered
        // recording that closed with a very short body
        // (fib base case: `Lt`/`Jmp`/`Return1` = 3 ops,
        // binary_trees `make(0)`: 4 ops) is pathological.
        // Compiling + caching it pins `Proto.traces` to a
        // trace that the length gate will refuse to
        // dispatch (per `MIN_DISPATCHABLE_TRUNC_BODY_FLOOR
        // = 40`), AND blocks the back-edge / longer-call
        // path from re-recording the same head_pc (the
        // dedup `already_cached` check below short-
        // circuits). The fix: discard the short call-
        // triggered recording WITHOUT caching, and bias
        // the proto's `call_hot_count` back to
        // `THRESHOLD - HOT_RETRY_WINDOW` so the next
        // sequence of calls retries the trigger at a
        // different (hopefully deeper) recursion point.
        //
        // Back-edge triggered traces are exempt — a
        // tight numeric-for loop's body is legitimately
        // 3 ops (`Add`, ForLoop) and DOES dispatch
        // usefully when re-entered many times.
        // Coverage heuristic to detect
        // pathologically partial call-triggered traces:
        // for self-recursive / branchy protos like
        // `fib` (~17 bytecode ops) or
        // `binary_trees.make` (~26 ops), the recorder
        // can fire at a BASE-case entry (`fib(0)` or
        // `make(0)`) producing a 3–4 op trace that
        // covers a tiny fraction of the proto's code.
        // That trace is doomed by the length gate
        // post-compile AND blocks any longer follow-up
        // (the dedup `already_cached` check below). The
        // fix: discard call-triggered closes where
        // `rec.ops.len() * 2 < head_proto.code.len()`
        // (less than half the proto's bytecode), so the
        // back-edge / longer call path can take over.
        //
        // Why coverage > raw length:protos with
        // intrinsically short bodies (closure
        // factories: `Closure + Return1` = 2 ops,
        // simple wrappers: `LoadI + Return1` = 2 ops)
        // record 100% coverage even at length 2 — those
        // ARE legitimately short and the closure /
        // sunk-emit lowering paths make
        // them worth compiling. The heuristic admits
        // them. fib's `[Lt, Jmp, Return1]` (3 of ~17)
        // and make's `[Lt, Jmp, LoadI, Return1]` (4 of
        // ~26) get discarded.
        //
        // Back-edge triggered traces are unaffected —
        // a tight numeric-for body legitimately covers
        // 3 of ~3 proto ops it can dispatch from
        // (`Add + ForLoop`) and the recorder fires on
        // the back-edge, not call entry.
        //
        // `call_hot_count` is intentionally NOT reset
        // (an earlier draft tried `THRESHOLD - 32` but
        // caused active_trace contention with the
        // outer back-edge trigger — see
        // setlist_b_zero_with_call_c_zero_sunk_emits).
        // We give up on dispatching the pathological
        // shape on the same proto; the back-edge or a
        // longer call path on a deeper recursion point
        // can still record + cache a real trace.
        let proto_code_len = rec.head_proto.code.len();
        let is_partial_coverage = rec.ops.len() * 2 < proto_code_len;
        // Per-Proto discard cap. The relaxed
        // trigger condition (`c >= THRESHOLD &&
        // !already_cached`) means a Proto whose every
        // recording is partial-coverage will re-fire the
        // trigger every call indefinitely (1500+ in
        // `binary_trees`-pattern test). The cap stops
        // discarding after `MAX_DISCARDS_PER_PROTO` —
        // the next close falls through to compile (even
        // if partial), caches the trace, and the
        // `already_cached` short-circuit kills the
        // storm. Dispatch may still be refused
        // post-compile (length gate), but the recorder
        // stops churning.
        const MAX_DISCARDS_PER_PROTO: u32 = 5;
        let prior_discards = rec.head_proto.trace_discard_count.get();
        let cap_reached = prior_discards >= MAX_DISCARDS_PER_PROTO;
        // Flip the `gave_up` flag the
        // moment cap is reached (BEFORE the close-
        // dispatching branch below). The trigger gates
        // short-circuit on this flag, skipping the
        // RefCell + linear `already_cached` scan on
        // every subsequent call to this Proto. Useful
        // for `binary_trees_pattern`-class loads where
        // a single Proto sees ~20k calls post-cap.
        if cap_reached
            && rec.is_call_triggered
            && is_partial_coverage
            && !rec.head_proto.trace_gave_up.get()
        {
            rec.head_proto.trace_gave_up.set(true);
        }
        if rec.is_call_triggered && is_partial_coverage && !cap_reached {
            // Tally as closed (for visibility) but DROP
            // without compile/cache. Use the existing
            // closed-lens accumulator so probes can
            // observe the discarded shape.
            // Bump discard count BEFORE
            // dropping the recording so the next
            // close sees the updated counter.
            rec.head_proto.trace_discard_count.set(prior_discards + 1);
            self.jit.counters.closed += 1;
            self.jit
                .counters
                .closed_lens
                .push((rec.is_call_triggered, rec.ops.len()));
            // Partial-coverage discard close path.
            // `closed` + `closed_lens` alone can't separate
            // a real successful close from a discard tally,
            // so tag explicitly to keep the recorder-side
            // close-cause taxonomy single-source.
            self.jit
                .counters
                .bump_close_cause("partial-coverage-discard");
            self.jit.active_trace = None;
            // Continue with interp loop — don't
            // fall through to compile path.
            // The op at `pc` hasn't dispatched yet;
            // the outer loop iteration handles it.
        } else {
            self.trace_compile_closed();
        }
    }

    /// Compile the closed recording and cache the trace on its head proto.
    fn trace_compile_closed(&mut self) {
        let rec = self.jit.active_trace.as_mut().expect("recording");
        rec.closed = true;
        // Detach the closed record, then try
        // to compile it. Dedup by `head_pc`: a Proto
        // already carrying a CompiledTrace for this PC
        // skips recompile (the hot counter caps
        // re-recording at `u32::MAX / 2` anyway, but
        // explicit dedup keeps `Proto.traces` short
        // for the dispatcher's linear scan).
        //
        // On failure we just bump the failed counter
        // and drop the record.
        let head_pc_val = rec.head_pc;
        let closed_record = self
            .jit
            .active_trace
            .take()
            .expect("active_trace was Some this branch");
        self.jit.counters.closed += 1;
        self.jit
            .counters
            .closed_lens
            .push((closed_record.is_call_triggered, closed_record.ops.len()));
        // Cache the trace on the
        // recorder's *head proto*, not the current
        // closure's proto. For non-recursive
        // call-triggered traces, close fires after
        // `Return1` pops the callee frame — `cl` at
        // that point is the CALLER's closure, while
        // `closed_record.head_proto` is the CALLEE's
        // proto (the one we actually want the trace
        // to be discoverable from on the next call).
        // Self-recursive fib closed via depth-cap
        // mid-recursion, so `cl.proto == head_proto`
        // there, but only by coincidence.
        let head_proto = closed_record.head_proto;
        let already_cached = head_proto
            .traces
            .borrow()
            .iter()
            .any(|t| t.head_pc == head_pc_val);
        if !already_cached {
            // Internal-loop = true: the trace runs in
            // a native loop until a cmp side-exits, so
            // the dispatcher's per-entry marshal cost
            // amortizes across the whole run of
            // iterations the loop's recorded direction
            // stays valid. The lowerer auto-downgrades
            // to one-shot for cmp-less or Call-truncating
            // traces.
            // Side traces MUST NOT
            // internal-loop. The parent's recorded prefix
            // (ops at PCs < side trace's head_pc) defines
            // values for registers the child's body reads
            // without re-writing each iter — e.g. for
            // s12_step_b, parent's `pc=19 Add R[12] = R[1]
            // + R[11]` sets R[12], and the child trace
            // (head_pc=24) re-runs `pc=20 Move R[1] =
            // R[12]` each iter via its outer ForLoop
            // internal-loop, ALWAYS reading the stale
            // entry-time R[12]. The parent's Add never
            // re-runs during child's loop, so R[1] gets
            // pinned to one stale value. Force one-shot
            // for side traces: each parent-exit round-
            // trips through dispatcher → parent's Add
            // runs → side trace runs ONE iter → return.
            let opts = crate::jit::trace::CompileOptions {
                internal_loop: closed_record.side_trace_parent.is_none(),
                pre53: self.version() <= LuaVersion::Lua53,
                aot: false,
                tier: self.jit.trace_tier,
                tier_up_at: self.jit.tier_up_at,
            };
            // Route through trace_compiler; split-borrow JitState
            // so the trait method can take `&mut dyn JitStorage`.
            let result = {
                let version = self.version();
                let jit = &mut self.jit;
                jit.storage.claim(self.jit_owner_id);
                let storage: &mut dyn crate::jit::JitStorage = jit.storage.as_mut();
                jit.trace_compiler
                    .try_compile_trace_for(storage, &closed_record, opts, version)
            };
            match result {
                Some(mut ct) => {
                    self.tally_compiled_trace(&ct);
                    self.wire_side_trace(&mut ct, &closed_record, head_proto);
                    cache_trace(head_proto, ct);
                    self.jit.counters.compiled += 1;
                }
                None => {
                    self.jit.counters.compile_failed += 1;
                    note_trace_compile_failure(head_proto, closed_record.head_pc);
                    self.jit
                        .counters
                        .compile_failed_reasons
                        .push(self.jit.trace_compiler.last_compile_checkpoint());
                }
            }
        }
    }

    /// The counters a compiled trace contributes to.
    fn tally_compiled_trace(&mut self, ct: &CompiledTrace) {
        // Tally Sinkable sites
        // + actually-sunk-emit sites + materialise
        // emit sites before moving `ct` into
        // Proto.traces.
        self.jit.counters.sinkable_seen += ct.sinkable_sites_seen as u64;
        self.jit.counters.accum_bufferable_seen += ct.accum_bufferable_seen as u64;
        self.jit.counters.sunk_alloc += ct.sunk_alloc_seen as u64;
        self.jit.counters.materialize_emit += ct.materialize_emit_count as u64;
        self.jit.counters.closure_emit += ct.closure_seen as u64;
        if ct.is_inline_abort_close {
            self.jit.counters.inline_abort += 1;
        }
        // Split tally so a
        // probe can answer the AOT
        // `accepted_with_per_exit_inline`
        // gate's question at the JIT
        // surface too: how many compiled
        // traces emitted depth>0 cmp
        // side-exits, and how many of
        // those survived all the
        // `dispatchable = false` pins
        // (`InlineAbort-gate`,
        // `self-link-retf-r1`,
        // `downrec-stitch-pending`, etc.).
        if !ct.per_exit_inline.is_empty() {
            self.jit.counters.per_exit_inline_compiled += 1;
            if ct.dispatchable {
                self.jit.counters.per_exit_inline_dispatchable += 1;
            }
        }
        if let Some(reason) = ct.dispatch_off_reason {
            self.jit.counters.dispatch_off_reasons.push(reason);
            // Mirror
            // the ordered Vec push into
            // the per-reason HashMap so
            // probes can answer "how many
            // of each dispatch_off label
            // fired" in O(1) without
            // walking the Vec. Same
            // bucket as the recorder-side
            // abort/discard tags above.
            self.jit.counters.bump_close_cause(reason);
        }
        // Count compiled traces that
        // carry a down-recursion stitch
        // link. Bumped here (not at the
        // lowerer emit site) because the
        // Vm's JitCounters live on the Vm,
        // and the lowerer doesn't have a
        // Vm handle. Read via
        // `Vm::trace_downrec_link_compiled_count`.
        if ct.downrec_link.is_some() {
            self.jit.counters.downrec_link_compiled += 1;
        }
        // Multi-way guard emit counter.
        // Bumped when the lowerer collected
        // >= 2 distinct caller_pc candidates
        // and lifted `dispatchable=true`.
        // The single-CMP shape stores
        // `1` here without bumping; non-
        // DownRec closes store `0`.
        if ct.downrec_multi_way_count >= 2 {
            self.jit.counters.multi_way_guard_emitted += 1;
        }
    }

    /// A closed side trace: hook its entry into the parent trace's exit.
    fn wire_side_trace(
        &mut self,
        ct: &mut CompiledTrace,
        closed_record: &TraceRecord,
        head_proto: Gc<crate::runtime::function::Proto>,
    ) {
        // Side-trace finalisation.
        // Pin `dispatchable=false` so the
        // primary lookup `traces.find(|t|
        // t.head_pc == pc && t.dispatchable)`
        // never matches this entry — the
        // side trace is meant to be entered
        // ONLY through the parent's exit
        // indirection, not the
        // back-edge / call-trigger paths.
        // Then write the entry fn ptr into
        // the parent's `exit_side_trace_ptrs`
        // slot so the parent's IR can read it.
        if let Some((parent_proto, parent_head_pc, parent_exit_idx)) =
            closed_record.side_trace_parent
        {
            // The lowerer's own verdict: a trace it
            // compiled but would not dispatch (an
            // untyped table read, an inline abort)
            // is not safe to enter from the parent's
            // exit either — unless the only reason
            // was the length gate, which weighs
            // dispatch overhead, not soundness (the
            // lowerer records the other reasons
            // first).
            let runnable = ct.dispatchable || ct.dispatch_off_reason == Some("length-gate");
            ct.dispatchable = false;
            let entry_ptr = ct.entry as *const () as *const u8;
            let _side_trace_head_pc = closed_record.head_pc;
            let parent_traces = parent_proto.traces.borrow();
            if let Some(parent_ct) = parent_traces.iter().find(|t| t.head_pc == parent_head_pc) {
                // Shape-match
                // gate. Find the parent's per-exit
                // tag snapshot at the wired exit
                // (inline / tag / global) and
                // check the child's entry_tags
                // match. If not, leave the cell
                // null + skip cache populate so
                // the parent IR's
                // `call_indirect` stays inert at
                // this exit (the child's
                // shape-specialised IR would
                // mis-interpret raw bits the
                // parent writes to reg_state).
                let inline_n = parent_ct.per_exit_inline.len();
                let tags_n = parent_ct.per_exit_tags.len();
                let parent_exit_tags_slice: &[crate::jit::trace::ExitTag] =
                    if parent_exit_idx < inline_n {
                        &parent_ct.per_exit_inline[parent_exit_idx].exit_tags
                    } else if parent_exit_idx < inline_n + tags_n {
                        &parent_ct.per_exit_tags[parent_exit_idx - inline_n].1
                    } else {
                        &parent_ct.exit_tags
                    };
                // A child that reads a slot the parent neither checked on
                // entry nor writes finds its value on the stack: the
                // dispatcher checks that slot's tag before running the child
                // (`child_reads_stack_held_ok`).
                let shape_matches = crate::jit::trace::exit_tags_match_entry_tags(
                    &ct.entry_tags,
                    parent_exit_tags_slice,
                    &stack_held_as_child(
                        &parent_ct.entry_tags,
                        &parent_ct.body_writes,
                        &ct.entry_tags,
                    ),
                );
                if !shape_matches {
                    self.jit.counters.side_trace_shape_mismatch += 1;
                }
                let shape_ok = runnable && shape_matches;
                // Write the child's
                // entry fn ptr to BOTH the legacy
                // `exit_side_trace_ptrs[idx]`
                // cell (read by the
                // walk_any_side_ptr_non_null tests)
                // AND the per-kind cell
                // whose heap address the parent's
                // IR baked. The IR-baked
                // cell is what the call_indirect
                // gate actually reads. Only write
                // when the shape gate passes.
                if shape_ok {
                    // a child that may move to the optimizing tier updates
                    // these cells then
                    let remember = |k: usize, cell: &crate::jit::send_compat::TCellPtr| {
                        if let Some(t) = &ct.tier_up {
                            t.parent_cells[k].set(cell as *const _ as *const u8);
                        }
                    };
                    if let Some(cell) = parent_ct.exit_side_trace_ptrs.get(parent_exit_idx) {
                        cell.set(entry_ptr);
                        remember(0, cell);
                    }
                    // Compute (kind, local) for the
                    // IR-baked cell. Layout follows
                    // exit_hit_counts: inline first,
                    // then per_exit_tags, then the
                    // global tail slot.
                    let (sent_kind, sent_local) = if parent_exit_idx < inline_n {
                        let cell = &parent_ct.per_exit_inline[parent_exit_idx].side_trace_ptr;
                        cell.set(entry_ptr);
                        remember(1, cell);
                        (
                            crate::jit::trace::SIDE_SENT_KIND_INLINE,
                            parent_exit_idx as u32,
                        )
                    } else if parent_exit_idx < inline_n + tags_n {
                        let local = parent_exit_idx - inline_n;
                        if let Some(b) = parent_ct.tags_side_trace_ptrs.get(local) {
                            b.set(entry_ptr);
                            remember(1, b);
                        }
                        (crate::jit::trace::SIDE_SENT_KIND_TAG, local as u32)
                    } else {
                        parent_ct.global_side_trace_ptr.set(entry_ptr);
                        remember(1, &parent_ct.global_side_trace_ptr);
                        (crate::jit::trace::SIDE_SENT_KIND_GLOBAL, 0)
                    };
                    self.jit.counters.side_trace_compiled += 1;
                    // Flip the
                    // parent's fast-path hint so
                    // the dispatcher knows to do
                    // the tentative decode + cell
                    // check on subsequent
                    // dispatches. Set once and
                    // stays true (we never unwire
                    // a side trace today).
                    parent_ct.has_any_side_wired.set(true);

                    // Populate
                    // the O(1) lookup cache the
                    // dispatcher consults on
                    // sentinel-bit-set returns.
                    // Key is the encoded sentinel
                    // (same encoding the IR ORs
                    // into bits 56..=62 of the
                    // child's i64 return).
                    let sentinel = crate::jit::trace::encode_side_sentinel(sent_kind, sent_local);
                    let predicted_idx = if std::ptr::eq(parent_proto.as_ptr(), head_proto.as_ptr())
                    {
                        parent_traces.len() as u32
                    } else {
                        head_proto.traces.borrow().len() as u32
                    };
                    parent_ct
                        .side_trace_cache
                        .borrow_mut()
                        .insert(sentinel, predicted_idx);
                }
            }
            drop(parent_traces);
        }
    }
}

/// The parent's entry tags as the shape gate compares them: a slot the
/// parent does not check on entry and never writes takes the child's tag,
/// which the dispatcher checks against the stack before it runs the child.
fn stack_held_as_child(parent_entry: &[u8], parent_writes: &[u32], child_entry: &[u8]) -> Vec<u8> {
    parent_entry
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            if t == crate::jit::trace::ENTRY_TAG_ANY && !parent_writes.contains(&(i as u32)) {
                child_entry.get(i).copied().unwrap_or(t)
            } else {
                t
            }
        })
        .collect()
}
