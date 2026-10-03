use super::*;

/// The loop edge at `effective_end`.
pub(super) fn emit_loop_tail<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    for_loop_idx: usize,
) -> Option<()> {
    let Plan { record, .. } = *pl;
    // ForLoop is only set at depth=0 (ForLoop@d>0 closes via
    // InlineAbort), so `regs_full[a]` directly addresses the
    // caller window — no offset.
    let rop = &record.ops[for_loop_idx];
    let a = rop.inst.a() as usize;
    match rop.inst.op() {
        Op::ForLoop => emit_for_loop_tail(lw, pl, rop, a)?,
        Op::TForLoop => emit_tfor_loop_tail(lw, pl, for_loop_idx, rop, a)?,
        _ => unreachable!("for_loop_idx_opt only set for Op::ForLoop / Op::TForLoop"),
    }
    Some(())
}

/// How a numeric `for` loop's state registers are laid out and stepped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ForForm {
    /// 5.4+ integer loop: R[A+1] is the unsigned count of iterations left.
    IntCount,
    /// 5.3 integer loop: R[A+1] is the limit, the index wraps on overflow.
    IntLimit,
    /// Float loop, every dialect: R[A+1] is the limit.
    Float,
}

/// The form of the loop whose state registers have kinds `idx`, `lim`
/// and `step` at its `ForLoop`, or `None` when the trace cannot step it.
/// 5.1 / 5.2 hold integer state only after `debug.setlocal` (their
/// `ForPrep` makes every loop a float loop), and stepping it in machine
/// integers would skip the doubles' rounding: such a loop is left to the
/// interpreter.
pub(super) fn for_form(pre53: bool, float_only: bool, kinds: [RegKind; 3]) -> Option<ForForm> {
    match kinds {
        [RegKind::Float, RegKind::Float, RegKind::Float] => Some(ForForm::Float),
        [RegKind::Int, RegKind::Int, RegKind::Int] if float_only => None,
        [RegKind::Int, RegKind::Int, RegKind::Int] if pre53 => Some(ForForm::IntLimit),
        [RegKind::Int, RegKind::Int, RegKind::Int] => Some(ForForm::IntCount),
        _ => None,
    }
}

/// `Op::ForLoop`, in the form its state registers' kinds select:
///
///   IntCount: continue while R[A+1] != 0; R[A] += R[A+2]; R[A+1] -= 1
///   IntLimit / Float: next = R[A] + R[A+2];
///     continue while (0 < step ? next <= limit : limit <= next);
///     R[A] = next
///
/// On continue R[A+3] = R[A] and the trace goes back to the loop body;
/// on exit it leaves at forloop.pc + 1 with the registers unchanged
/// (PUC writes nothing when the loop ends).
pub(super) fn emit_for_loop_tail<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    rop: &RecordedOp,
    a: usize,
) -> Option<()> {
    let Plan {
        record,
        max_stack,
        do_internal_loop,
        opts,
        float_only,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        ..
    } = *lw;
    let kinds = [
        lw.current_kinds[a],
        lw.current_kinds[a + 1],
        lw.current_kinds[a + 2],
    ];
    let form = for_form(opts.pre53, float_only, kinds)?;
    let cur = lw.bcx.use_var(lw.regs_full[a]);
    let x = lw.bcx.use_var(lw.regs_full[a + 1]);
    let step = lw.bcx.use_var(lw.regs_full[a + 2]);
    let (cond, next) = match form {
        ForForm::IntCount => {
            let zero = lw.bcx.ins().iconst(types::I64, 0);
            // the loop count is unsigned (PUC `lua_Unsigned`)
            (lw.bcx.ins().icmp(IntCC::NotEqual, x, zero), None)
        }
        ForForm::IntLimit => {
            let next = lw.bcx.ins().iadd(cur, step);
            let up = lw.bcx.ins().icmp_imm_s(IntCC::SignedGreaterThan, step, 0);
            let le = lw.bcx.ins().icmp(IntCC::SignedLessThanOrEqual, next, x);
            let ge = lw.bcx.ins().icmp(IntCC::SignedGreaterThanOrEqual, next, x);
            (lw.bcx.ins().select(up, le, ge), Some(next))
        }
        ForForm::Float => {
            let f = |lw: &mut Lower<E>, v| lw.bcx.ins().bitcast(types::F64, MemFlagsData::new(), v);
            let (cur_f, lim_f, step_f) = (f(lw, cur), f(lw, x), f(lw, step));
            let next_f = lw.bcx.ins().fadd(cur_f, step_f);
            let zero = lw.bcx.ins().f64const(0.0);
            // a NaN anywhere fails both comparisons and ends the loop
            let up = lw.bcx.ins().fcmp(FloatCC::LessThan, zero, step_f);
            let le = lw.bcx.ins().fcmp(FloatCC::LessThanOrEqual, next_f, lim_f);
            let ge = lw.bcx.ins().fcmp(FloatCC::LessThanOrEqual, lim_f, next_f);
            let next = lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), next_f);
            (lw.bcx.ins().select(up, le, ge), Some(next))
        }
    };

    // the caller window: see `emit_tail`
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    let continue_blk = lw.bcx.create_block();
    let exit_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(cond, continue_blk, &[], exit_blk, &[]);

    // exit branch: side-exit at forloop.pc + 1.
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        caller_regs,
        &lw.stored,
        reg_state,
        rop.pc + 1,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );

    // continue branch: step the index and go back.
    lw.bcx.switch_to_block(continue_blk);
    lw.bcx.seal_block(continue_blk);
    let next = match next {
        Some(next) => next,
        None => {
            let next = lw.bcx.ins().iadd(cur, step);
            let one = lw.bcx.ins().iconst(types::I64, 1);
            let count_new = lw.bcx.ins().isub(x, one);
            lw.bcx.def_var(lw.regs_full[a + 1], count_new);
            next
        }
    };
    lw.bcx.def_var(lw.regs_full[a], next);
    lw.bcx.def_var(lw.regs_full[a + 3], next);
    // ForLoop's continue branch jumps to the loop's
    // BODY START (= (rop.pc + 1) - bx per OP_FORLOOP's
    // backward jump encoding), not record.head_pc.
    // For trace shapes whose head_pc == body_start
    // (the usual back-edge trace), they're equal.
    // For side traces whose head_pc lands on the
    // ForLoop op itself (head_pc=rop.pc) instead of
    // the back-edge target — e.g. an outer ForLoop
    // that got recorded as a side trace from an inner
    // loop exit — returning record.head_pc would
    // re-enter the ForLoop op and double-advance the
    // counter. A trace headed at an inner loop (a
    // `while` inside the for body) that closes at the
    // outer ForLoop must not loop back to its own head
    // either: that skips the body code before the inner
    // loop. Compute the body start explicitly.
    let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
    let mut tail_kinds = lw.current_kinds[..max_stack].to_vec();
    tail_kinds[a + 3] = kinds[0];
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

/// `Op::TForLoop`, the generic-for back-edge.
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
    let nvars = match record.ops[for_loop_idx - 1].inst.op() {
        Op::TForCall => record.ops[for_loop_idx - 1].inst.c() as usize,
        _ => return None,
    };
    let key_tag = *record.entry_tags.get(a + 4)?;
    let val_tag = if nvars >= 2 {
        Some(*record.entry_tags.get(a + 5)?)
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
    for k in (a + 4)..(a + 4 + nvars).min(nil_snapshot.len()) {
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
    let ctrl = lw.bcx.use_var(lw.regs_full[a + 4]);
    lw.bcx.def_var(lw.regs_full[a + 2], ctrl);
    // as for ForLoop: continue at the loop body, which is
    // the trace head only when the trace was recorded from it
    let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
    // the loop variables passed the tag check above, and the
    // control variable is a copy of the key
    let mut tail_kinds = lw.current_kinds[..max_stack].to_vec();
    let vars = (a + 4)..(a + 4 + nvars.min(2)).min(max_stack);
    tail_kinds[vars.clone()].copy_from_slice(&lw.head_kinds[vars.clone()]);
    tail_kinds[a + 2] = lw.head_kinds[a + 4];
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

/// The edge back to the loop head. With an iteration count to keep, the
/// trace leaves at its head once the count is reached; the dispatcher then
/// moves it to the optimizing tier.
pub(super) fn emit_back_edge<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        body_loop,
        ..
    } = *lw;
    sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
    let Some((cell, at)) = &lw.tier_count else {
        lw.bcx.ins().jump(body_loop, &[]);
        return;
    };
    let (cell, at) = (&**cell as *const TCellU32 as i64, *at);
    let hot = lw.bcx.create_block();
    lw.bcx.tier_count(cell, at, hot, body_loop);
    lw.bcx.switch_to_block(hot);
    lw.bcx.seal_block(hot);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        &lw.regs_full[..max_stack],
        &lw.stored,
        reg_state,
        record.head_pc,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );
}
