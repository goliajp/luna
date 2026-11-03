//! The kinds a trace's loop edge leaves, and the dispatch gates that need
//! the whole trace.

use super::*;

/// The kinds the loop edge leaves and the dispatch gates that need the
/// whole trace.
pub(super) fn apply_tail_kinds_and_gates(
    pl: &Plan<'_>,
    mut current_kinds: Vec<RegKind>,
    mut dispatchable: bool,
    mut dispatch_off_reason: Option<&'static str>,
    per_exit_inline_vec: &[(u32, u32, Vec<RegKind>, TArc<[FrameMaterializeInfo]>)],
    sunk_alloc_seen: u32,
) -> (Vec<RegKind>, bool, Option<&'static str>) {
    let Plan {
        record,
        effective_end,
        call_idx_opt,
        for_loop_idx_opt,
        inline_abort_idx_opt,
        return_idx_opt,
        ..
    } = *pl;
    // a tail ForLoop writes R[A], R[A+3] and (5.4+) R[A+1], all of
    // R[A]'s kind (it compiles only when R[A..=A+2] share one); a
    // TForLoop's R[A+2] = R[A+4] is already in `current_kinds`
    if let Some(for_loop_idx) = for_loop_idx_opt {
        let rop = &record.ops[for_loop_idx];
        let a = rop.inst.a() as usize;
        if rop.inst.op().plain_for_loop() == Op::ForLoop {
            current_kinds[a + 3] = current_kinds[a];
        }
    }
    // Derive exit_tags from the kind tracker's final state. Slots
    // the trace never touched stay `Untouched` (dispatcher restores
    // the entry tag); slots the trace wrote take the writer's
    // determined kind. `current_kinds` propagates source kinds at
    // the Move op so the dispatcher doesn't need a deferred
    // entry-tag lookup.
    // dispatch heuristic: a `Op::Call`-truncated
    // trace whose body is too short to amortise the dispatcher's
    // marshal-in + transmute + restore overhead is a net loss vs the
    // interpreter (measured at ~1.8× slower on fib_28's ~7-op body).
    // Keep such traces cached (compile cost is paid) but pin
    // dispatchable=false unless the per-dispatch body is large
    // enough to win. `MIN_DISPATCHABLE_TRUNC_BODY_BASE` is tuned to fib's
    // 7-op body being just below the gate at depth=0.
    //
    // scale the gate down as `max_depth_used` grows:
    // each extra inline level amortises ~2 ops worth of marshal
    // overhead per dispatch (one dispatch processes the full
    // chain of depth+1 frames). Saturating-sub so deep traces
    // never miss-fire on the length gate.
    const MIN_DISPATCHABLE_TRUNC_BODY_BASE: usize = 20;
    // floor at 40 ops/dispatch (the empirical
    // dispatcher-overhead amortisation line: ~80ns per dispatch /
    // ~2ns per body op). The adaptive `BASE - depth*2`
    // formula alone could drop the gate to 0 at MAX_INLINE_DEPTH=16,
    // letting tiny-body inline traces dispatch and pay overhead
    // they can't amortise (binary_trees_d4 runs 0.73× without the
    // floor). The floor doesn't affect fib_28 (~112 ops body —
    // well above the floor) but bails the binary_trees
    // pathological case.
    const MIN_DISPATCHABLE_TRUNC_BODY_FLOOR: usize = 40;
    let max_depth_used = record
        .ops
        .iter()
        .map(|r| r.inline_depth as usize)
        .max()
        .unwrap_or(0);
    let adaptive = MIN_DISPATCHABLE_TRUNC_BODY_BASE.saturating_sub(max_depth_used * 2);
    let min_dispatchable_trunc_body = adaptive.max(MIN_DISPATCHABLE_TRUNC_BODY_FLOOR);
    // inline traces (per_exit_metas non-empty)
    // skip the length-gate. Each dispatch tears through multiple
    // inlined frames so body-length isn't a useful proxy for the
    // dispatcher's marshal overhead; the gate would dump fib's
    // ~8-op-by-the-time-MAX_DEPTH-hits prefix even though one
    // dispatch processes 4 recursion levels.
    //
    // sunk-alloc traces also skip the length-gate.
    // Skipping even a single `Heap::new_table()` per dispatch
    // dwarfs the marshal-in/out overhead on a 7-op body, so the
    // gate's conservative default is a net loss here.
    //
    // Closure-creating traces do NOT skip the
    // length-gate. Unlike sunk emit which avoids `Heap::new_table()`,
    // the Op::Closure helper still calls `Heap::new_closure_inline`
    // — emit replaces only the interp's match-arm dispatch +
    // frame plumbing for the 2-op `Closure + Return1` shape, which
    // is less than trace dispatch's marshal+enter overhead. Per-iter
    // dispatch of a tiny closure-constructor body is a net loss
    // (probe: `closure_no_upval_for_500k` mac measured 0.53× when
    // the gate was skipped). Closure traces only earn dispatch when
    // body length passes the gate organically.
    // InlineAbort traces close without materialising frames. The
    // interp can't resume at the inline-abort PC without the
    // matching CallFrames, so gate dispatch off.
    // Recorded before the length gate: the first reason is the one
    // kept, and side-trace wiring tells a trace that is unsafe to run
    // from one that is only too short to dispatch by it.
    if inline_abort_idx_opt.is_some() {
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("InlineAbort-gate"));
    }
    if (call_idx_opt.is_some() || return_idx_opt.is_some())
        && effective_end < min_dispatchable_trunc_body
        && per_exit_inline_vec.is_empty()
        && sunk_alloc_seen == 0
    {
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("length-gate"));
    }

    (current_kinds, dispatchable, dispatch_off_reason)
}
