use super::*;

/// What the emit pass leaves for `CompiledTrace`.
pub(super) struct Emitted {
    pub(super) current_kinds: Vec<RegKind>,
    pub(super) dispatchable: bool,
    pub(super) dispatch_off_reason: Option<&'static str>,
    pub(super) per_exit_kinds: Vec<(u32, Vec<RegKind>, Box<TCellPtr>)>,
    pub(super) per_exit_inline_vec: Vec<(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )>,
    pub(super) sunk_alloc_seen: u32,
    pub(super) materialize_emit_count: u32,
    pub(super) closure_seen: u32,
    pub(super) escape: EscapeAnalysis,
    pub(super) global_side_trace_box: Box<TCellPtr>,
    pub(super) downrec_link_for_compiled: Option<(u32, u32)>,
    pub(super) downrec_multi_way_count_for_compiled: u8,
    pub(super) tier_count: Option<(Box<TCellU32>, u32)>,
}

/// The kinds the loop edge leaves and the dispatch gates that need the
/// whole trace.
fn apply_tail_kinds_and_gates(
    pl: &Plan<'_>,
    mut current_kinds: Vec<RegKind>,
    mut dispatchable: bool,
    mut dispatch_off_reason: Option<&'static str>,
    per_exit_inline_vec: &[(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )],
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
    // Op::ForLoop at the tail writes R[A] (next loop var), R[A+3]
    // (visible loop var copy) and, in the 5.4+ count form, R[A+1]
    // (decremented count), each of R[A]'s kind: the tail only
    // compiles when R[A..=A+2] share one number kind. Op::TForLoop
    // writes R[A+2] = R[A+4] on continue, of the key's kind, which the
    // generic-for tail already left in `current_kinds`.
    if let Some(for_loop_idx) = for_loop_idx_opt {
        let rop = &record.ops[for_loop_idx];
        let a = rop.inst.a() as usize;
        if rop.inst.op() == Op::ForLoop {
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

/// Builds the `CompiledTrace` from the emit pass's bookkeeping.
pub(super) fn build_compiled(pl: &Plan<'_>, em: Emitted) -> CompiledTrace {
    let Emitted {
        current_kinds,
        dispatchable,
        dispatch_off_reason,
        per_exit_kinds,
        per_exit_inline_vec,
        sunk_alloc_seen,
        materialize_emit_count,
        closure_seen,
        escape,
        global_side_trace_box,
        downrec_link_for_compiled,
        downrec_multi_way_count_for_compiled,
        tier_count,
    } = em;
    let (current_kinds, dispatchable, dispatch_off_reason) = apply_tail_kinds_and_gates(
        pl,
        current_kinds,
        dispatchable,
        dispatch_off_reason,
        &per_exit_inline_vec,
        sunk_alloc_seen,
    );
    let Plan {
        record,
        max_stack,
        window_size,
        inline_abort_idx_opt,
        ..
    } = *pl;
    // clean-tail `exit_tags` cover the caller window
    // only ([0..max_stack)). Per-side-exit `per_exit_tags` for inline
    // cmp sites carry the full `window_size` snapshot
    // because the dispatcher must restore EVERY pushed frame's
    // register window, not just the caller's.
    let mut exit_tags_vec = kinds_to_exit_tags(&current_kinds[..max_stack]);
    // for every sunk site at depth=0 (depth>0 is rejected
    // in pre-emit), force the slot's exit tag to `Untouched` so the
    // dispatcher carries the entry tag in the restore. Without this
    // override the slot's `current_kinds` could read as Table (from
    // some other path) or Unset, and the dispatcher would try to
    // unpack `reg_state[a]` (which we never wrote for sunk sites)
    // as a `Value::Table` of NULL bits → SIGSEGV.
    for site in &escape.sites {
        if site.state == EscapeState::Sinkable && site.inline_depth == 0 {
            let idx = site.a as usize;
            if idx < exit_tags_vec.len() {
                exit_tags_vec[idx] = ExitTag::Untouched;
            }
        }
    }
    let global_tag_res_kind = classify_exit_tags(&exit_tags_vec);
    let exit_tags: TArc<[ExitTag]> = exit_tags_vec.into();
    // split per_exit_kinds's 3-tuple into the
    // 2-tuple `per_exit_tags` for the dispatcher AND the parallel
    // `tags_side_trace_ptrs` Box slice the close handler writes to.
    // The Box transports the cell's heap address (baked into the
    // IR's `iconst` at each callsite) through this move without
    // moving the cell itself.
    let mut tags_side_boxes: Vec<Box<TCellPtr>> = Vec::with_capacity(per_exit_kinds.len());
    let per_exit_tags: TArc<[(u32, TArc<[ExitTag]>)]> = per_exit_kinds
        .into_iter()
        .map(|(pc, kinds, side_box)| {
            // The cmp emit site pushed the right slice
            // length (caller-window for depth=0, full window for
            // depth>0). Hand it through verbatim — the dispatcher
            // iterates `exit_tags_for_pc.len()` and walks both
            // shapes uniformly.
            let tags: TArc<[ExitTag]> = kinds_to_exit_tags(&kinds).into();
            tags_side_boxes.push(side_box);
            (pc, tags)
        })
        .collect::<Vec<_>>()
        .into();
    let tags_side_trace_ptrs: TArc<[Box<TCellPtr>]> = tags_side_boxes.into();
    let per_exit_inline: TArc<[InlineSideExit]> = per_exit_inline_vec
        .into_iter()
        .map(
            |(cont_pc, head_resume_pc, kinds, chain, side_trace_ptr)| InlineSideExit {
                cont_pc,
                head_resume_pc,
                exit_tags: kinds_to_exit_tags(&kinds).into(),
                chain,
                side_trace_ptr,
            },
        )
        .collect::<Vec<_>>()
        .into();

    checkpoint("post:emit-pass-done");
    // pre-compute exit_hit_counts before the struct
    // init so per_exit_tags's len is still accessible.
    let exit_hit_counts: TArc<[TCellU32]> = {
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let v: Vec<TCellU32> = (0..total).map(|_| TCellU32::new(0)).collect();
        v.into()
    };
    // parallel per-exit raw fn-ptr slots, all null
    // until a child side trace compiles for the slot. Same length
    // as exit_hit_counts.
    let exit_side_trace_ptrs: TArc<[TCellPtr]> = {
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let v: Vec<TCellPtr> = (0..total).map(|_| TCellPtr::null()).collect();
        v.into()
    };
    CompiledTrace {
        head_pc: record.head_pc,
        // caller (JIT wrapper or AOT pipeline)
        // patches `entry` after finalize. See [`placeholder_trace_fn`].
        entry: placeholder_trace_fn,
        n_ops: record.ops.len() as u32,
        dispatchable,
        // real window_size ≥ max_stack; the
        // dispatcher reads this to size its reg_state buffer.
        window_size,
        exit_tags,
        global_tag_res_kind,
        is_inline_abort_close: inline_abort_idx_opt.is_some(),
        dispatch_off_reason: if dispatchable {
            None
        } else {
            dispatch_off_reason
        },
        entry_tags: record
            .entry_tags
            .iter()
            .enumerate()
            .map(|(i, &t)| match pl.head_live.get(i) {
                Some(false) => ENTRY_TAG_ANY,
                _ => luna_core::jit::trace::entry_tag_of(t),
            })
            .collect::<Vec<u8>>()
            .into(),
        per_exit_tags,
        // populated by the cmp@d>0 emit sites
        // above; the IR encodes `(site_idx + 1)` in the upper 32
        // bits of its return value so the dispatcher can pull the
        // right entry. Holding the inner Rc<[FrameMaterializeInfo]>
        // alive keeps each chain's address stable across dispatches
        // (cranelift IR has the raw pointer baked in via iconst).
        exit_hit_counts,
        exit_side_trace_ptrs,
        // per-TAG-entry side-trace cells (parallel
        // to per_exit_tags) + the GLOBAL singleton cell. Both
        // collected from Boxes allocated AT each emit callsite so
        // the IR has baked the right heap address.
        tags_side_trace_ptrs,
        global_side_trace_ptr: global_side_trace_box,
        // empty at compile; close handler fills
        // it as child side traces compile for this trace's hot
        // exits.
        side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
        has_any_side_wired: TCellBool::new(false),
        per_exit_inline,
        // diagnostic only; counts Sinkable sites from the
        // pre-emit sweep. Vm sums these into
        // `trace_sinkable_seen_count`.
        sinkable_sites_seen: escape.sinkable_count(),
        accum_bufferable_seen: escape
            .accum_sites
            .iter()
            .filter(|s| s.state == BufferState::Bufferable)
            .count() as u32,
        // count of sites that actually took the sunk-emit
        // path in this trace's body (NewTable replaced by virt slot
        // Variables, no heap alloc helper called). Vm bumps
        // `trace_sunk_alloc_count` by this on compile success.
        sunk_alloc_seen,
        // count of materialise emit sites for sunk slot
        // recovery at cmp side-exits.
        materialize_emit_count,
        // count of Op::Closure ops the trace lowered.
        closure_seen,
        // compute body_writes for the smart side-trace
        // gate. Uses op_offsets (already computed above) to apply
        // inline-depth offsets per op.
        body_writes: compute_body_writes(record, &pl.op_offsets).into(),
        // populated by the `downrec_idx_opt` arm
        // above into `downrec_link_for_compiled`. When the arm
        // emitted a stitch sentinel + caller-pc guard, this carries
        // `Some((0, head_pc))`; otherwise `None`. `Some(_)` alone
        // doesn't make the trace dispatchable — that takes a
        // multi-way candidate count >= 2 (see
        // `downrec_multi_way_count` below).
        downrec_link: downrec_link_for_compiled,
        // multi-way guard candidate count baked
        // into the IR's CMP-chain. `0` for non-DownRec closes;
        // `1` for single-CMP-fallback DownRec; `>= 2` for the
        // lifted `dispatchable = true` path.
        downrec_multi_way_count: downrec_multi_way_count_for_compiled,
        tier_up: tier_count
            .map(|(count, at)| tier_up(count, at, record.head_proto.call_hot_count.get())),
    }
}

/// The record of a baseline trace that moves to Cranelift after `at`
/// iterations and entries, counted in `count` (fewer once its function,
/// called `calls_at` times so far, is called again).
fn tier_up(count: Box<TCellU32>, at: u32, calls_at: u32) -> Box<TierUp> {
    Box::new(TierUp {
        count,
        at,
        calls_at,
        optimized: TCellPtr::null(),
        tried: TCellBool::new(false),
        parent_cells: [TCellPtr::null(), TCellPtr::null()],
        source: TRefLock::new(None),
    })
}
