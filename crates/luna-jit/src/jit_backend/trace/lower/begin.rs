use super::*;

/// The emit-pass state at the loop head.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin_body<E: Emit>(
    bcx: E,
    pl: &Plan<'_>,
    h: Helpers,
    head: EntryRegs,
    escape: EscapeAnalysis,
    sunk: (Vec<Option<Vec<Variable>>>, Vec<Option<Vec<RegKind>>>, u32),
    flush_ctx: Option<FlushCtx>,
    blocks: HeadBlocks,
) -> Lower<E> {
    let (virt_vars, virt_kinds, sunk_alloc_seen) = sunk;
    let current_kinds = entry_kinds(pl);
    let ro_invariant = ro_invariants(pl, &blocks, &current_kinds);
    Lower {
        bcx,
        h,
        entry: head,
        blocks,
        back_edge_kinds: None,
        head_kinds: current_kinds[..pl.max_stack].to_vec(),
        defined_aot_data: std::collections::HashSet::new(),
        escape,
        flush_ctx,
        virt_vars,
        virt_kinds,
        sunk_alloc_seen,
        materialize_emit_count: 0,
        closure_seen: 0,
        stored: Vec::new(),
        at_exits: false,
        head_stored: Vec::new(),
        current_kinds,
        dispatchable: true,
        dispatch_off_reason: None,
        // about one exit per recorded op
        per_exit_kinds: ExitKinds::with_capacity(pl.record.ops.len(), pl.max_stack),
        per_exit_inline_vec: Vec::new(),
        call_chain: Vec::new(),
        upvals: UpvalReads::default(),
        known: Known::new(pl.window_size_us + NVIRT),
        virt_held: Vec::new(),
        alt_joins: std::collections::HashMap::new(),
        tier_count: None,
        write_only_kinds: Ok(Vec::new()),
        counts_per_call: false,
        loop_entry: None,
        loops: false,
        due_words: None,
        gc_quiet: false,
        ro_invariant,
        ro_checked: Vec::new(),
        side_cells: Vec::new(),
        relink: None,
    }
}

/// [`readonly_invariants`] when the trace tests tables before the loop head, else none.
fn ro_invariants(pl: &Plan<'_>, blocks: &HeadBlocks, current_kinds: &[RegKind]) -> Vec<bool> {
    match blocks.ro_precheck {
        Some(_) => readonly_invariants(pl, current_kinds),
        None => vec![false; pl.window_size_us],
    }
}

/// The kinds the registers hold at the loop head (see `Lower::current_kinds`).
fn entry_kinds(pl: &Plan<'_>) -> Vec<RegKind> {
    (0..pl.window_size_us + NVIRT)
        .map(|i| match pl.head_live.get(i) {
            Some(true) => pl
                .record
                .entry_tags
                .get(i)
                .and_then(|&t| RegKind::from_entry_tag(t))
                .unwrap_or(RegKind::Unset),
            Some(false) => RegKind::StackHeld,
            None => RegKind::Unset,
        })
        .collect()
}
