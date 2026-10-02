use super::*;

/// Emits the tail, the way the trace closes, and seals the loop head.
/// Returns the down-recursion link and guard count for `CompiledTrace`.
pub(super) fn emit_tail<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
) -> Option<(Option<(u32, u32)>, u8)> {
    let Plan {
        record,
        max_stack,
        call_idx_opt,
        for_loop_idx_opt,
        inline_abort_idx_opt,
        return_idx_opt,
        self_link_idx_opt,
        downrec_idx_opt,
        do_internal_loop,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        body_loop,
        ..
    } = *lw;
    // --- tail.
    //
    // Four cases pick the clean-close shape:
    //
    // - Trace truncated by `Op::Call` → store back + return
    //   `call.pc`. The Call's interp re-execution is the
    //   "exit" — no loop possible.
    // - Trace closes on `Op::ForLoop` (5.4+ Int count form) →
    //   emit the count check + step IR, then either jump back
    //   to `body_loop` (continue path) or side-exit at
    //   `forloop.pc + 1` (loop exit). In one-shot mode the
    //   continue path returns `head_pc` instead, so the
    //   dispatcher gets one iter per entry.
    // - `opts.internal_loop && has_cmp` (and no Call truncation,
    //   no ForLoop) → jump back to `body_loop`. The trace runs
    //   natively until some cmp side-exits; the dispatcher's
    //   per-entry marshal cost amortizes across however many
    //   iterations the loop runs.
    // - Otherwise (one-shot mode or a no-cmp trace) → store back
    //   + return `head_pc`. The dispatcher re-enters per
    //   iteration; an internal loop with no side-exit would
    //   spin forever.
    // every tail `emit_store_back_and_return_pc`
    // passes `&regs_full[..max_stack]` so the store-back ONLY writes
    // the caller's window back to interp stack. Slots at
    // [max_stack..window_size) are inline-frame scratch and must not
    // leak into the dispatcher's reg_state restore.
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    // populated by the `downrec_idx_opt` arm when
    // it emits the stitch sentinel. Flows into `CompiledTrace.
    // downrec_link` at the struct literal below. `None` for every
    // other close shape.
    let mut downrec_link_for_compiled: Option<(u32, u32)> = None;
    let mut downrec_multi_way_count_for_compiled: u8 = 0;
    if let Some((_dr_idx, dr_return_pc, _target_proto_id, _depth_delta)) = downrec_idx_opt {
        (
            downrec_link_for_compiled,
            downrec_multi_way_count_for_compiled,
        ) = emit_downrec_tail(lw, pl, dr_return_pc, _target_proto_id, _depth_delta);
    } else if let Some((_self_link_idx, _kind)) = self_link_idx_opt {
        emit_self_link_tail(lw, pl);
    } else if let Some(call_idx) = call_idx_opt {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.ops[call_idx].pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(inline_abort_idx) = inline_abort_idx_opt {
        // InlineAbort: emit-up-to-i, then store back
        // + return record.ops[i].pc. Dispatchable is forced false
        // below (the interp can't resume at a depth>0 PC without the
        // CallFrames the trace inlined past).
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.ops[inline_abort_idx].pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(return_idx) = return_idx_opt {
        // Return0/Return1 at depth=0: caller frame
        // unwinds. Same shape as Call truncation — store back caller
        // window + return the Return op's PC so the interp re-executes
        // it with the correct register state. Subject to the same
        // length-gate dispatchable check below.
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.ops[return_idx].pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(for_loop_idx) = for_loop_idx_opt {
        emit_loop_tail(lw, pl, for_loop_idx)?;
    } else if do_internal_loop && loop_kinds_match(&lw.current_kinds[..max_stack], &lw.head_kinds) {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
        lw.bcx.ins().jump(body_loop, &[]);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.head_pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    }
    // Seal the loop head now that both predecessors are emitted
    // (entry → body_loop in the prelude; tail → body_loop from
    // whichever block we ended up in for the clean-close case
    // when internal loop is on).
    lw.bcx.seal_block(body_loop);
    Some((
        downrec_link_for_compiled,
        downrec_multi_way_count_for_compiled,
    ))
}
