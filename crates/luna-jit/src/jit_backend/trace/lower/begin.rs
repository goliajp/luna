use super::*;

/// The emit-pass state at the loop head.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin_body<E: Emit>(
    bcx: E,
    pl: &Plan<'_>,
    h: Helpers,
    head: EntryRegs,
    escape: EscapeAnalysis,
    defined_aot_data: std::collections::HashSet<DataId>,
    sunk: (Vec<Option<Vec<Variable>>>, Vec<Option<Vec<RegKind>>>, u32),
    flush_ctx: Option<FlushCtx>,
    blocks: (Option<Block>, Option<Block>, Option<Block>, Block),
) -> Lower<E> {
    let EntryRegs {
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
    } = head;
    let (virt_vars, virt_kinds, sunk_alloc_seen) = sunk;
    let (precheck, ro_precheck, step_precheck, body_loop) = blocks;
    let Plan {
        record,
        max_stack,
        window_size_us,
        ..
    } = *pl;
    let current_kinds = entry_kinds(pl);
    let head_kinds = current_kinds[..max_stack].to_vec();
    let ro_invariant = match ro_precheck {
        Some(_) => readonly_invariants(pl, &current_kinds),
        None => vec![false; window_size_us],
    };
    Lower {
        bcx,
        h,
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
        precheck,
        ro_precheck,
        step_precheck,
        body_loop,
        head_kinds,
        defined_aot_data,
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
        per_exit_kinds: ExitKinds::with_capacity(record.ops.len(), max_stack),
        per_exit_inline_vec: Vec::new(),
        call_chain: Vec::new(),
        upval_cache: std::collections::HashMap::new(),
        upval_checked: std::collections::HashMap::new(),
        upval_check_done: Vec::new(),
        head_closure_var: None,
        known_int: vec![None; window_size_us + NVIRT],
        const_str: vec![false; window_size_us + NVIRT],
        virt_held: Vec::new(),
        alt_joins: std::collections::HashMap::new(),
        tier_count: None,
        due_words: None,
        gc_quiet: false,
        ro_invariant,
        ro_checked: Vec::new(),
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
