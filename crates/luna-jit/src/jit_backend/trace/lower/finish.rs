use super::*;
use luna_core::__private::{PassCount, TracePlans};
use std::sync::Arc;

mod gates;
use gates::{apply_tail_kinds_and_gates, exit_tag_table, inline_kinds};

/// What the emit pass leaves for `CompiledTrace`.
pub(super) struct Emitted {
    pub(super) current_kinds: Vec<RegKind>,
    pub(super) dispatchable: bool,
    pub(super) dispatch_off_reason: Option<&'static str>,
    pub(super) per_exit_kinds: ExitKinds,
    pub(super) per_exit_inline_vec: Vec<(u32, u32, Vec<RegKind>, TArc<[FrameMaterializeInfo]>)>,
    pub(super) sunk_alloc_seen: u32,
    pub(super) materialize_emit_count: u32,
    pub(super) closure_seen: u32,
    pub(super) escape: EscapeAnalysis,
    pub(super) downrec_link_for_compiled: Option<(u32, u32)>,
    pub(super) downrec_multi_way_count_for_compiled: u8,
    pub(super) tier_count: Option<Box<TierUp>>,
    /// See `Lower::write_only_kinds`.
    pub(super) write_only_kinds: Result<Vec<(u16, ExitTag)>, ()>,
    /// See `Lower::counts_per_call`.
    pub(super) counts_per_call: bool,
    /// See `side_call::place_cells`.
    pub(super) side_cells: (Arc<[u32]>, Option<luna_core::__private::SideCells>),
    /// The head frame's kinds at the back edge into the optimizing tier's
    /// loop, for a trace with one.
    pub(super) loop_kinds: Option<Vec<RegKind>>,
    /// What the expansion of the instructions takes (see `relinks`).
    pub(super) relinks: Option<super::Relinks>,
}

impl<E: Emit> Lower<E> {
    /// The builder and what the emit pass decided, once the trace is
    /// lowered; `downrec`: what `emit_tail` returned, `relinks` what the
    /// expansion takes, which makes the cells.
    pub(super) fn into_emitted(
        self,
        downrec: (Option<(u32, u32)>, u8),
        relinks: Option<super::Relinks>,
    ) -> (E, Emitted) {
        let Lower {
            entry,
            mut bcx,
            current_kinds,
            dispatchable,
            dispatch_off_reason,
            per_exit_kinds,
            per_exit_inline_vec,
            sunk_alloc_seen,
            materialize_emit_count,
            closure_seen,
            escape,
            tier_count,
            loop_entry,
            write_only_kinds,
            counts_per_call,
            side_cells,
            back_edge_kinds,
            loops,
            ..
        } = self;
        let side_cells = match &relinks {
            Some(x) => (x.cells.clone(), None),
            None => super::side_call::place_cells(&mut bcx, side_cells),
        };
        if let Some(at) = loop_entry {
            bcx.set_loop_entry(at, &entry.regs_full);
        }
        (
            bcx,
            Emitted {
                current_kinds,
                dispatchable,
                dispatch_off_reason,
                per_exit_kinds,
                per_exit_inline_vec,
                sunk_alloc_seen,
                materialize_emit_count,
                closure_seen,
                escape,
                downrec_link_for_compiled: downrec.0,
                downrec_multi_way_count_for_compiled: downrec.1,
                tier_count,
                write_only_kinds,
                counts_per_call,
                side_cells,
                loop_kinds: back_edge_kinds.filter(|_| loops),
                relinks,
            },
        )
    }
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
        downrec_link_for_compiled,
        downrec_multi_way_count_for_compiled,
        tier_count,
        write_only_kinds,
        counts_per_call,
        side_cells,
        loop_kinds,
        relinks: _,
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
    // `per_exit_tags` for the dispatcher. Each exit's tags keep the length its emit site pushed (caller window at
    // depth 0, the full window deeper): the dispatcher walks
    // `exit_tags_for_pc.len()` either way.
    let per_exit_tags = exit_tag_table(per_exit_kinds.iter());
    let inline_tags =
        exit_tag_table((per_exit_inline_vec.iter()).map(|(pc, _, kinds, _)| (*pc, &kinds[..])));
    let per_exit_inline: TArc<[InlineSideExit]> = per_exit_inline_vec
        .into_iter()
        .map(|(cont_pc, head_resume_pc, _, chain)| InlineSideExit {
            cont_pc,
            head_resume_pc,
            chain,
        })
        .collect();
    let pass_count = pass_count(counts_per_call, write_only_kinds.unwrap_or_default());

    checkpoint("post:emit-pass-done");
    let entry_tags: Vec<u8> = record
        .entry_tags
        .iter()
        .enumerate()
        .map(|(i, &t)| match pl.head_live.get(i) {
            Some(false) => ENTRY_TAG_ANY,
            _ => luna_core::jit::trace::entry_tag_of(t),
        })
        .collect();
    let plans = TracePlans::new(
        &entry_tags,
        &exit_tags_vec,
        per_exit_tags,
        inline_tags,
        pass_count,
    );
    let exit_hit_counts: TArc<[TCellU32]> =
        (0..plans.exit_count()).map(|_| TCellU32::new(0)).collect();
    CompiledTraceParts {
        head_pc: record.head_pc,
        // caller (JIT wrapper or AOT pipeline)
        // patches `entry` after finalize. See [`placeholder_trace_fn`].
        entry: placeholder_trace_fn,
        n_ops: record.ops.len() as u32,
        dispatchable,
        // real window_size ≥ max_stack; the dispatcher reads this to size its reg_state buffer
        window_size,
        plans,
        is_inline_abort_close: inline_abort_idx_opt.is_some(),
        dispatch_off_reason: if dispatchable {
            None
        } else {
            dispatch_off_reason
        },
        // populated by the cmp@d>0 emit sites
        // above; the IR encodes `(site_idx + 1)` in the upper 32
        // bits of its return value so the dispatcher can pull the
        // right entry. Holding the inner Rc<[FrameMaterializeInfo]>
        // alive keeps each chain's address stable across dispatches
        // (cranelift IR has the raw pointer baked in via iconst).
        exit_hit_counts,
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
        tier_up: tier_count,
        side_cell_exits: side_cells.0,
        side_cells: side_cells.1,
        calls_source: None,
        link_pc: record.link_pc,
        loop_tags: loop_kinds.map(|k| kinds_to_exit_tags(&k[..max_stack]).into()),
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

/// Where the code counts its passes: in this call's words when a back edge
/// counts there, which is how a trace with written-only registers counts
/// (see `handoff::count_at`), with their kinds past the back edge.
fn pass_count(per_call: bool, write_only: Vec<(u16, ExitTag)>) -> PassCount {
    if per_call {
        PassCount::PerCall(write_only.into())
    } else {
        debug_assert!(
            write_only.is_empty(),
            "written-only registers, not counted per call"
        );
        PassCount::Shared
    }
}
