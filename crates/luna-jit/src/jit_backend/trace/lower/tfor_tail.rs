use super::*;

/// A generic `for`'s `TForLoop`, the back-edge. Below, A+4 stands for
/// the first loop variable and A+2 for the control register (5.4's layout;
/// `ForLayout` gives the others).
pub(super) fn emit_tfor_loop_tail<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    for_loop_idx: usize,
    rop: &RecordedOp,
    a: usize,
) -> Option<()> {
    let Plan {
        record,
        max_stack,
        do_internal_loop,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        tforcall_tag_var,
        tforcall_val_tag_var,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    // the caller window: see `emit_tail`
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    // generic-for back-edge:
    //
    //   tag = tforcall_tag_var  // from TForCall's
    //                           //     batched helper
    //                           //     return value
    //   if tag == NIL:  side-exit at tforloop.pc + 1
    //   elif the key's (and value's) tag is the one the
    //        body was compiled for: R[A+2]=R[A+4] +
    //        back-edge
    //   else: deopt (the interpreter runs the TForLoop)
    //
    // The Nil branch reuses the existing dispatcher
    // restore path; push a per_exit_kinds snapshot with
    // [A+4] = RegKind::Nil so the Nil side-exit repacks
    // correctly (entry's tag for A+4 was Int, so
    // dispatcher without override would restore as Int
    // — wrong for Nil).
    let tag = lw.bcx.use_var(tforcall_tag_var);
    // The body was lowered for the head's entry tags; the
    // back-edge runs it again only with a key (and, when the
    // loop has one, a value) of those tags. A pairs loop
    // over string keys meeting an integer key (or the
    // reverse) stored the new key under the old tag.
    let call = record.ops[for_loop_idx - 1].inst;
    if !call.op().is_tfor_call() {
        return None;
    }
    let nvars = call.c() as usize;
    let lay = call.op().for_layout()?;
    let first = a + lay.var() as usize;
    let key_tag = *record.entry_tags.get(first)?;
    let val_tag = if nvars >= 2 {
        Some(*record.entry_tags.get(first + 1)?)
    } else {
        None
    };

    let nil_const = lw
        .bcx
        .ins()
        .iconst(types::I64, luna_core::runtime::value::raw::NIL as i64);
    let is_nil = lw.bcx.ins().icmp(IntCC::Equal, tag, nil_const);
    let nil_exit_blk = lw.bcx.create_block();
    let not_nil_blk = lw.bcx.create_block();
    lw.bcx
        .ins()
        .brif(is_nil, nil_exit_blk, &[], not_nil_blk, &[]);

    // Nil-exit branch: snapshot per_exit_kinds with [A+4]
    // = Nil, then store back + return tforloop.pc + 1.
    lw.bcx.switch_to_block(nil_exit_blk);
    lw.bcx.seal_block(nil_exit_blk);
    // Every loop variable restores as nil: the key is nil, and
    // the value slots hold what the iterator's last call left
    // (nil in the helper path), which the loop no longer reads.
    let mut nil_snapshot: Vec<RegKind> = lw.current_kinds[..max_stack].to_vec();
    for k in first..(first + nvars).min(nil_snapshot.len()) {
        nil_snapshot[k] = RegKind::Nil;
    }
    let tag_side_box_2: Box<TCellPtr> = Box::new(TCellPtr::null());
    let _tag_side_cell_addr_2 = (&*tag_side_box_2) as *const TCellPtr as i64;
    let tag_side_local_2 = lw.per_exit_kinds.len() as u32;
    lw.per_exit_kinds
        .push((rop.pc + 1, nil_snapshot, tag_side_box_2));
    emit_tagged_exit(
        &mut lw.bcx,
        suppress_admit_id,
        caller_regs,
        &lw.stored,
        reg_state,
        rop.pc + 1,
        record.head_pc,
        tag_side_local_2,
        lw.flush_ctx.as_ref(),
        trace_fn_sig_ref,
    );

    lw.bcx.switch_to_block(not_nil_blk);
    lw.bcx.seal_block(not_nil_blk);
    let mut same_kinds = lw
        .bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, tag, i64::from(key_tag));
    if let Some(val_tag) = val_tag {
        let v = lw.bcx.use_var(tforcall_val_tag_var);
        let same_val = lw.bcx.ins().icmp_imm_u(IntCC::Equal, v, i64::from(val_tag));
        same_kinds = lw.bcx.ins().band(same_kinds, same_val);
    }
    let continue_blk = lw.bcx.create_block();
    let deopt_blk = lw.bcx.create_block();
    lw.bcx
        .ins()
        .brif(same_kinds, continue_blk, &[], deopt_blk, &[]);

    // Deopt: the next key or value has another kind than the
    // body was compiled for. Store back + return TForLoop.pc
    // so the interp re-executes the back-edge.
    lw.bcx.switch_to_block(deopt_blk);
    lw.bcx.seal_block(deopt_blk);
    // The helper already wrote the loop variables to the stack
    // with their tags, which are not the ones the registers
    // were compiled for; the dispatcher must leave them there.
    emit_store_back_and_return(
        &mut lw.bcx,
        caller_regs,
        &lw.stored,
        reg_state,
        (luna_core::jit::trace_types::EXIT_KEEP_TFOR_VARS | u64::from(rop.pc)) as i64,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );

    // Continue: R[A+2] = R[A+4] (ctrl writeback) +
    // back-edge / store_back+head_pc.
    lw.bcx.switch_to_block(continue_blk);
    lw.bcx.seal_block(continue_blk);
    if lay.copies_control() {
        let ctrl = lw.bcx.use_var(lw.regs_full[first]);
        lw.bcx.def_var(lw.regs_full[a + 2], ctrl);
    }
    // as for ForLoop: continue at the loop body, which is
    // the trace head only when the trace was recorded from it
    let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
    // the loop variables passed the tag check above, and the
    // control variable is a copy of the key
    let mut tail_kinds = lw.current_kinds[..max_stack].to_vec();
    let vars = first..(first + nvars.min(2)).min(max_stack);
    tail_kinds[vars.clone()].copy_from_slice(&lw.head_kinds[vars.clone()]);
    if lay.copies_control() {
        tail_kinds[a + 2] = lw.head_kinds[first];
    }
    // the return below is the trace's clean tail, whose exit tags come
    // from the kinds the emit pass ends with: the loop variables hold the
    // next iteration's values, which the interpreter must get back
    lw.current_kinds[vars.clone()].copy_from_slice(&tail_kinds[vars]);
    lw.current_kinds[a + 2] = tail_kinds[a + 2];
    if do_internal_loop
        && body_pc == record.head_pc
        && loop_kinds_match(&tail_kinds, &lw.head_kinds)
    {
        emit_back_edge(lw, pl);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            body_pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    }
    Some(())
}
