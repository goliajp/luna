use super::*;
use luna_core::__private::classify_exit_tags;

mod gates;
use gates::apply_tail_kinds_and_gates;

/// What the emit pass leaves for `CompiledTrace`.
pub(super) struct Emitted {
    pub(super) current_kinds: Vec<RegKind>,
    pub(super) dispatchable: bool,
    pub(super) dispatch_off_reason: Option<&'static str>,
    pub(super) per_exit_kinds: ExitKinds,
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

/// Builds the `CompiledTrace` from the emit pass's bookkeeping.
pub(super) fn build_compiled(pl: &Plan<'_>, em: Emitted) -> CompiledTraceParts {
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
    // `per_exit_tags` for the dispatcher, and the parallel
    // `tags_side_trace_ptrs` cells the close handler writes to. Each
    // exit's tags keep the length its emit site pushed (caller window at
    // depth 0, the full window deeper): the dispatcher walks
    // `exit_tags_for_pc.len()` either way.
    let tag_vecs: Vec<(u32, Vec<ExitTag>)> = per_exit_kinds
        .iter()
        .map(|(pc, kinds)| (pc, kinds_to_exit_tags(kinds)))
        .collect();
    let per_exit_tags = ExitTagTable::from_exits(tag_vecs.iter().map(|(pc, t)| (*pc, &t[..])));
    let tags_side_trace_ptrs: Box<[TCellPtr]> =
        (0..per_exit_tags.len()).map(|_| TCellPtr::null()).collect();
    let mut last = None;
    let inline_tags: Vec<TArc<[ExitTag]>> = per_exit_inline_vec
        .iter()
        .map(|(_, _, kinds, _, _)| shared_exit_tags(kinds, &mut last))
        .collect();
    let per_exit_inline: TArc<[InlineSideExit]> = per_exit_inline_vec
        .into_iter()
        .zip(inline_tags)
        .map(
            |((cont_pc, head_resume_pc, _, chain, side_trace_ptr), exit_tags)| InlineSideExit {
                cont_pc,
                head_resume_pc,
                exit_tags,
                chain,
                side_trace_ptr,
            },
        )
        .collect();

    checkpoint("post:emit-pass-done");
    // before the struct, while per_exit_tags's len can be read
    let exits = per_exit_inline.len() + per_exit_tags.len() + 1;
    let exit_hit_counts: TArc<[TCellU32]> = (0..exits).map(|_| TCellU32::new(0)).collect();
    // per exit, the side trace's entry once one compiles (null before)
    let exit_side_trace_ptrs: TArc<[TCellPtr]> = (0..exits).map(|_| TCellPtr::null()).collect();
    CompiledTraceParts {
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
        // to per_exit_tags) + the GLOBAL singleton cell
        tags_side_trace_ptrs,
        global_side_trace_ptr: global_side_trace_box,
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
        inline_kinds: inline_kinds(pl),
        inline_depth: record.ops.iter().map(|o| o.inline_depth).max().unwrap_or(0),
        // compute body_writes for the smart side-trace
        // gate. Uses op_offsets (already computed above) to apply
        // inline-depth offsets per op.
        body_writes: body_writes(pl).into(),
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
        tier_up: tier_count.map(|(count, at)| {
            Box::new(TierUp::new(count, at, record.head_proto.call_hot_count()))
        }),
    }
}

/// The registers the trace's ops write, the values inlined functions
/// return and vararg expansions included, sorted.
fn body_writes(pl: &Plan<'_>) -> Vec<u32> {
    let mut w = compute_body_writes(pl.record, &pl.op_offsets);
    for &(first, n) in &pl.inline_writes {
        w.extend(first..first + n);
    }
    w.sort_unstable();
    w.dedup();
    w
}

/// The kinds of inlined code the trace holds (`INLINE_*`).
fn inline_kinds(pl: &Plan<'_>) -> u8 {
    let mut k = 0;
    for (i, rop) in pl.record.ops[..pl.effective_end].iter().enumerate() {
        if let Some(c) = pl.inline_calls.get(i).copied().flatten() {
            let callee = pl.record.ops[i + 1].proto;
            if callee.is_vararg {
                k |= INLINE_VARARG_CALLEE;
            }
            if !(0..=1).contains(&c.nresults) {
                k |= INLINE_MULTI_RESULTS;
            }
            if rop.inst.b() == 0 {
                k |= INLINE_VAR_ARGS;
            }
        }
        if rop.inline_depth > 0 && rop.inst.op() == Op::Closure {
            k |= INLINE_CLOSURE;
        }
    }
    k
}
