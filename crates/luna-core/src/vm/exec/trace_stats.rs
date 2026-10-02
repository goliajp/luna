//! Read-only trace JIT counters for tests and diagnostics.

use super::*;

impl Vm {
    /// Number of traces that have closed cleanly (looped back to the
    /// head PC) since this Vm was constructed. Cumulative; used by
    /// tests + tuning.
    pub fn trace_closed_count(&self) -> u64 {
        self.jit.counters.closed
    }

    /// Number of traces that have aborted (exceeded MAX_TRACE_LEN or
    /// hit an un-recordable op).
    pub fn trace_aborted_count(&self) -> u64 {
        self.jit.counters.aborted
    }

    /// Number of compiled traces whose close shape
    /// is `TraceEnd::InlineAbort` (depth>0 boundary). Such traces
    /// pin `dispatchable=false` because the dispatcher can't
    /// resume at a depth>0 PC without the matching CallFrames.
    /// The frame-materialisation helper could synthesise those, but
    /// the InlineAbort emit path isn't wired up to it.
    pub fn trace_inline_abort_count(&self) -> u64 {
        self.jit.counters.inline_abort
    }

    /// See `JitCounters::dispatch_off_reasons`.
    pub fn trace_dispatch_off_reasons(&self) -> &[&'static str] {
        &self.jit.counters.dispatch_off_reasons
    }

    /// See `JitCounters::compile_failed_reasons`.
    pub fn trace_compile_failed_reasons(&self) -> &[&'static str] {
        &self.jit.counters.compile_failed_reasons
    }

    /// See `JitCounters::closed_lens`. Returns
    /// `(is_call_triggered, ops_len)` for every trace that closed.
    pub fn trace_closed_lens(&self) -> &[(bool, usize)] {
        &self.jit.counters.closed_lens
    }

    /// See [`crate::vm::jit_state::JitCounters::close_cause_counts`].
    /// Per-reason close-cause counts (recorder-side abort/discard +
    /// lowerer-side dispatch_off labels) keyed by `&'static str`.
    pub fn trace_close_cause_counts(&self) -> &std::collections::HashMap<&'static str, u64> {
        &self.jit.counters.close_cause_counts
    }

    /// Number of compiled traces whose
    /// `CompiledTrace.downrec_link` is `Some(_)` (lowerer's
    /// `downrec_idx_opt` arm emitted the stitch sentinel + caller-pc
    /// guard scaffold).
    pub fn trace_downrec_link_compiled_count(&self) -> u64 {
        self.jit.counters.downrec_link_compiled
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::downrec_dispatched`]. Number
    /// of times the dispatcher's `is_downrec_sentinel` arm fired and
    /// classified the return as a caller-pc-guard HIT.
    pub fn trace_downrec_dispatched_count(&self) -> u64 {
        self.jit.counters.downrec_dispatched
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::downrec_deopt`]. Number of
    /// times the dispatcher entered a `downrec_link`-bearing trace and
    /// the trace returned via the lowerer's deopt block (caller-pc
    /// guard MISS), or the dispatcher itself force-deopted via the
    /// stitch-cycle checkpoint.
    pub fn trace_downrec_deopt_count(&self) -> u64 {
        self.jit.counters.downrec_deopt
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::multi_way_guard_emitted`].
    /// Number of compiled traces whose lowerer emitted a multi-way
    /// caller-pc guard chain (>= 2 distinct `caller_pc` candidates)
    /// at the `TraceEnd::DownRec` close + lifted `dispatchable = true`.
    pub fn trace_multi_way_guard_emitted_count(&self) -> u64 {
        self.jit.counters.multi_way_guard_emitted
    }

    /// Number of closed traces the lowerer compiled and
    /// parked on `Proto.traces`. Re-records of the same head_pc are
    /// deduped (the second close finds the head_pc already cached
    /// and skips compile), so this never exceeds `trace_closed_count`.
    pub fn trace_compiled_count(&self) -> u64 {
        self.jit.counters.compiled
    }

    /// Number of times the recorder captured a
    /// [`crate::jit::trace_types::FieldIcSnapshot`] with the field IC
    /// switch on ([`Self::set_field_ic_enabled`]). Stays 0 while it is
    /// off.
    pub fn trace_field_ic_snapshot_count(&self) -> u64 {
        self.jit.counters.field_ic_snapshot_captured
    }

    /// Number of closed traces the lowerer rejected
    /// (any of the bail conditions in
    /// `crate::jit::trace::try_compile_trace`).
    pub fn trace_compile_failed_count(&self) -> u64 {
        self.jit.counters.compile_failed
    }

    /// Number of times the dispatcher jumped into a
    /// compiled trace. Bumps on every entry; `trace_deopt_count`
    /// counts the subset where the trace returned with a parked
    /// `jit_pending_err`.
    pub fn trace_dispatched_count(&self) -> u64 {
        self.jit.counters.dispatched
    }

    /// Number of trace entries that came back with
    /// `jit_pending_err` set (typically a metatable shadowed an
    /// index inside a helper, forcing the dispatcher to fall back
    /// to the interpreter without committing the trace's result).
    pub fn trace_deopt_count(&self) -> u64 {
        self.jit.counters.deopt
    }

    /// Number of times the dispatcher started a side
    /// trace recording (an `exit_hit_counts` slot crossed
    /// [`crate::jit::trace::HOTEXIT_THRESHOLD`] while `active_trace`
    /// was None and trace JIT was enabled). Each unit is exactly one
    /// `start_side_trace` call; the actual compile success counts
    /// under [`Self::trace_compiled_count`] like any other trace.
    /// Probe use: distinguishes the "side-trace pipeline fired"
    /// signal from the "primary back-edge / call-trigger fired"
    /// signal without reading per-counter histograms.
    pub fn trace_side_trace_started_count(&self) -> u64 {
        self.jit.counters.side_trace_started
    }

    /// Number of side-trace recordings that closed,
    /// compiled successfully, AND patched their parent's
    /// `exit_side_trace_ptrs[exit_idx]`.
    pub fn trace_side_trace_compiled_count(&self) -> u64 {
        self.jit.counters.side_trace_compiled
    }

    /// Number of side traces that compiled
    /// successfully but were SHEDDED by the close-handler shape-
    /// match gate (`exit_tags_match_entry_tags`). High ratios
    /// vs. `trace_side_trace_compiled_count` indicate the
    /// architecture is shedding lots of would-be side traces;
    /// useful as a tuning probe for future relaxation of the
    /// gate or for child-IR re-specialisation against parent's
    /// exit shape.
    pub fn trace_side_trace_shape_mismatch_count(&self) -> u64 {
        self.jit.counters.side_trace_shape_mismatch
    }

    /// Sum of NewTable sites the pre-emit escape sweep
    /// classified as `crate::jit::trace::EscapeState::Sinkable`
    /// across every successfully compiled trace on this Vm. The
    /// count is post-demotion: sites pre-emit drops back to Escaped
    /// for not meeting the sunk-emit criteria are NOT counted.
    /// `trace_sunk_alloc_count` matches one-for-one today (every
    /// surviving Sinkable site goes through sunk emit).
    pub fn trace_sinkable_seen_count(&self) -> u64 {
        self.jit.counters.sinkable_seen
    }

    /// See `JitCounters::accum_bufferable_seen`.
    pub fn trace_accum_bufferable_seen_count(&self) -> u64 {
        self.jit.counters.accum_bufferable_seen
    }

    /// Total dispatch hits across all known traces,
    /// broken into hot-exit telemetry (max single-exit count,
    /// total dispatches, exit count). Used by probes to identify
    /// hot side-exits as side-trace candidates.
    ///
    /// Walks `cl.proto` AND all nested protos in `cl.proto.protos`
    /// recursively, so inner functions' traces are reported.
    pub fn trace_exit_hit_summary(
        &self,
        cl: crate::runtime::heap::Gc<crate::runtime::function::LuaClosure>,
    ) -> Vec<(u32, Vec<u32>)> {
        fn walk(
            proto: crate::runtime::heap::Gc<crate::runtime::function::Proto>,
            out: &mut Vec<(u32, Vec<u32>)>,
        ) {
            for ct in proto.traces.borrow().iter() {
                let counts: Vec<u32> = ct.exit_hit_counts.iter().map(|c| c.get()).collect();
                out.push((ct.head_pc, counts));
            }
            for inner in proto.protos.iter() {
                walk(*inner, out);
            }
        }
        let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
        walk(cl.proto, &mut out);
        out
    }

    /// Surface every side-exit slot whose hit count is
    /// `>= HOTEXIT_THRESHOLD` across every trace reachable from
    /// `cl.proto` (recursively walking `proto.protos`). Returned
    /// entries are side-trace candidates: each carries the parent
    /// trace's `(head_proto, head_pc)`, the exit's index in the
    /// parent's `exit_hit_counts`, and the side trace's natural
    /// entry shape (`cont_pc` + `exit_tags`).
    ///
    /// Layout of `exit_hit_counts` (mirrored by the iter):
    /// - `[0..per_exit_inline.len())` → `InlineSideExit` (cont_pc +
    ///   window-sized exit_tags).
    /// - `[per_exit_inline.len()..inline.len() + per_exit_tags.len())`
    ///   → `per_exit_tags[i]` (per-cont_pc caller-window tags).
    /// - Last slot → global clean-tail (cont_pc = `head_pc`,
    ///   exit_tags = `ct.exit_tags`).
    pub fn hot_exit_iter(
        &self,
        cl: crate::runtime::heap::Gc<crate::runtime::function::LuaClosure>,
    ) -> Vec<crate::jit::trace::HotExitInfo> {
        use crate::jit::trace::{HOTEXIT_THRESHOLD, HotExitInfo};
        fn walk(
            proto: crate::runtime::heap::Gc<crate::runtime::function::Proto>,
            out: &mut Vec<HotExitInfo>,
        ) {
            for ct in proto.traces.borrow().iter() {
                let inline_n = ct.per_exit_inline.len();
                let tags_n = ct.per_exit_tags.len();
                debug_assert_eq!(
                    ct.exit_hit_counts.len(),
                    inline_n + tags_n + 1,
                    "exit_hit_counts layout invariant violated"
                );
                for (idx, cell) in ct.exit_hit_counts.iter().enumerate() {
                    let hits = cell.get();
                    if hits < HOTEXIT_THRESHOLD {
                        continue;
                    }
                    let (cont_pc, exit_tags) = if idx < inline_n {
                        let ent = &ct.per_exit_inline[idx];
                        (ent.cont_pc, ent.exit_tags.clone())
                    } else if idx < inline_n + tags_n {
                        let (pc, tags) = &ct.per_exit_tags[idx - inline_n];
                        (*pc, tags.clone())
                    } else {
                        (ct.head_pc, ct.exit_tags.clone())
                    };
                    out.push(HotExitInfo {
                        head_proto: proto,
                        head_pc: ct.head_pc,
                        exit_idx: idx,
                        hits,
                        cont_pc,
                        exit_tags,
                    });
                }
            }
            for inner in proto.protos.iter() {
                walk(*inner, out);
            }
        }
        let mut out: Vec<HotExitInfo> = Vec::new();
        walk(cl.proto, &mut out);
        out
    }

    /// Sum of NewTable sites that actually took the
    /// sunk-emit path across every successfully compiled trace on
    /// this Vm. Each counted site skips its heap `Gc<Table>`
    /// allocation per dispatch; the array part lives as Cranelift
    /// `Variable`s for the duration of the trace.
    pub fn trace_sunk_alloc_count(&self) -> u64 {
        self.jit.counters.sunk_alloc
    }

    /// Sum of materialise-helper emit sites across every
    /// successfully compiled trace on this Vm. Each unit is a
    /// (site × cmp side-exit) pair whose IR reconstructs a heap
    /// `Gc<Table>` from the virt slots on deopt.
    pub fn trace_materialize_emit_count(&self) -> u64 {
        self.jit.counters.materialize_emit
    }

    /// Diagnostic: total `Op::Closure` ops the trace JIT
    /// lowered to the `luna_jit_op_closure` helper. Each emitted op
    /// replaces a `Heap::new_closure_inline` call on the dispatch
    /// path; the count is static (one per matching op per compiled
    /// trace), summed at compile success.
    pub fn trace_closure_emit_count(&self) -> u64 {
        self.jit.counters.closure_emit
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::per_exit_inline_compiled`].
    /// Number of compiled traces whose `per_exit_inline.len() > 0`
    /// (depth>0 inlined cmp side-exits emitted).
    pub fn trace_per_exit_inline_compiled_count(&self) -> u64 {
        self.jit.counters.per_exit_inline_compiled
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::per_exit_inline_dispatchable`].
    /// Number of compiled traces with `per_exit_inline.len() > 0` AND
    /// `dispatchable == true` — i.e. the count of compiled traces
    /// that would actually exercise the AOT chain-reloc +
    /// deploy-resolver path.
    pub fn trace_per_exit_inline_dispatchable_count(&self) -> u64 {
        self.jit.counters.per_exit_inline_dispatchable
    }

    /// Diagnostic: max `inline_depth` ever seen on any
    /// `RecordedOp` pushed by the recorder. Tells tests + tuning
    /// whether a self-recursive function actually walked the depth
    /// tracker past 0. Saturates at `MAX_INLINE_DEPTH`. Persists
    /// across traces and Vm activations; reset only on `Vm::new`.
    pub fn trace_max_depth_seen(&self) -> u8 {
        self.jit.max_depth_seen
    }
}
