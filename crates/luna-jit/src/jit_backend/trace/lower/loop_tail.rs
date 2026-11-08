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
        op if op.is_for_loop() => emit_for_loop_tail(lw, pl, rop, a)?,
        op if op.is_tfor_loop() => emit_tfor_loop_tail(lw, pl, for_loop_idx, rop, a)?,
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
/// (PUC writes nothing when the loop ends). 5.5's `ForLoop55` keeps the
/// count (or limit) in R[A], the step in R[A+1] and the index in R[A+2],
/// the loop variable itself.
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
        ..
    } = *pl;
    let reg_state = lw.entry.reg_state;
    let fs = for_step(lw, pl, rop, a, true)?;
    let cond = fs.cond;
    let continue_blk = lw.bcx.create_block();
    let exit_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(cond, continue_blk, &[], exit_blk, &[]);

    // exit branch: side-exit at forloop.pc + 1, a typed exit of its own
    // when it may run a side trace from the code (a loop the trace leaves
    // there often goes on into another compiled loop)
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    if exit_calls(lw, pl, rop.pc + 1) {
        let tags_idx = lw
            .per_exit_kinds
            .push(rop.pc + 1, &lw.current_kinds[..max_stack]);
        emit_calling_exit(lw, pl, rop.pc + 1, tags_idx);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            // the caller window: see `emit_tail`
            &lw.entry.regs_full[..max_stack],
            &lw.stored,
            reg_state,
            rop.pc + 1,
            lw.flush_ctx.as_ref(),
        );
    }

    // continue branch: step the index and go back.
    lw.bcx.switch_to_block(continue_blk);
    lw.bcx.seal_block(continue_blk);
    step_for(lw, &fs);
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
    if let Some(r) = fs.r_var {
        tail_kinds[r] = fs.index_kind;
    }
    if do_internal_loop
        && body_pc == record.head_pc
        && loop_kinds_match(&tail_kinds, &lw.head_kinds)
    {
        emit_back_edge(lw, pl, &tail_kinds);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            // the caller window: see `emit_tail`
            &lw.entry.regs_full[..max_stack],
            &lw.stored,
            reg_state,
            body_pc,
            lw.flush_ctx.as_ref(),
        );
    }
    Some(())
}

/// A numeric `for` loop's test and next index ([`emit_for_loop_tail`]).
pub(super) struct ForStep {
    /// Whether the loop goes on.
    pub(super) cond: Value,
    next: Option<Value>,
    cur: Value,
    x: Value,
    step: Value,
    r_cur: usize,
    r_x: usize,
    pub(super) r_var: Option<usize>,
    pub(super) index_kind: RegKind,
}

/// The test of the `ForLoop` `rop` (state registers from `a`). `ends`: it
/// ends the trace, whose step guard (the sign checked before the loop
/// head) is about this loop.
pub(super) fn for_step<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    rop: &RecordedOp,
    a: usize,
    ends: bool,
) -> Option<ForStep> {
    // the index, the count or limit, the step, and the loop variable when
    // it is a copy of the index
    let v55 = rop.inst.op().plain_for_loop() == Op::ForLoop55;
    let (r_cur, r_x, r_step, r_var) = if v55 {
        (a + 2, a, a + 1, None)
    } else {
        (a, a + 1, a + 2, Some(a + 3))
    };
    let kinds = [
        lw.current_kinds[r_cur],
        lw.current_kinds[r_x],
        lw.current_kinds[r_step],
    ];
    let form = for_form(pl.opts.pre53 && !v55, pl.float_only, kinds)?;
    let cur = lw.bcx.use_var(lw.entry.regs_full[r_cur]);
    let x = lw.bcx.use_var(lw.entry.regs_full[r_x]);
    let step = lw.bcx.use_var(lw.entry.regs_full[r_step]);
    let guard = pl.step_guard.filter(|&(r, _)| ends && r == r_step);
    let (cond, next) = match form {
        ForForm::IntCount => {
            let zero = lw.bcx.ins().iconst(types::I64, 0);
            // the loop count is unsigned (PUC `lua_Unsigned`)
            (lw.bcx.ins().icmp(IntCC::NotEqual, x, zero), None)
        }
        // the step's sign was checked before the loop head
        ForForm::IntLimit if guard.is_some() => {
            let next = lw.bcx.ins().iadd(cur, step);
            let cc = match guard {
                Some((_, true)) => IntCC::SignedLessThanOrEqual,
                _ => IntCC::SignedGreaterThanOrEqual,
            };
            (lw.bcx.ins().icmp(cc, next, x), Some(next))
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
            let next = lw
                .bcx
                .ins()
                .bitcast(types::I64, MemFlagsData::new(), next_f);
            (lw.bcx.ins().select(up, le, ge), Some(next))
        }
    };
    Some(ForStep {
        cond,
        next,
        cur,
        x,
        step,
        r_cur,
        r_x,
        r_var,
        index_kind: kinds[0],
    })
}

/// The loop going on: the index (and the loop variable) stepped, the count
/// of an integer loop counted down.
pub(super) fn step_for<E: Emit>(lw: &mut Lower<E>, fs: &ForStep) {
    let next = match fs.next {
        Some(next) => next,
        None => {
            let next = lw.bcx.ins().iadd(fs.cur, fs.step);
            let one = lw.bcx.ins().iconst(types::I64, 1);
            let count_new = lw.bcx.ins().isub(fs.x, one);
            lw.bcx.def_var(lw.entry.regs_full[fs.r_x], count_new);
            next
        }
    };
    lw.bcx.def_var(lw.entry.regs_full[fs.r_cur], next);
    if let Some(r) = fs.r_var {
        lw.bcx.def_var(lw.entry.regs_full[r], next);
    }
}

/// The edge back to the loop head, leaving the head-frame registers of
/// kinds `tail`: the kinds the loop's copy of the first pass starts with
/// (see `peel`). With an iteration count to keep, the trace goes on in the
/// optimizing tier's loop, or leaves at its head for the dispatcher to move
/// it there, once the count is reached (see `handoff`).
pub(super) fn emit_back_edge<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, tail: &[RegKind]) {
    let body_loop = lw.blocks.body_loop;
    lw.back_edge_kinds = Some(tail.to_vec());
    note_write_only_kinds(lw, pl, tail);
    commit_back_edge(lw);
    let tier = lw.tier_count.as_deref();
    let cx = ctx_of(&lw.entry, lw.flush_ctx.as_ref(), tier, pl);
    let Some(t) = cx.tier else {
        lw.bcx.ins().jump(body_loop, &[]);
        return;
    };
    lw.counts_per_call |= t.per_call();
    lw.loops = true;
    let hot = lw.bcx.create_block();
    lw.bcx.tier_count(t.count, t.at, hot, body_loop);
    switch_sealed(&mut lw.bcx, hot);
    emit_hot_back_edge(&mut lw.bcx, &cx, &lw.stored);
}

/// Notes the kinds `tail` leaves the written-only registers in (see
/// `Lower::write_only_kinds`).
fn note_write_only_kinds<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, tail: &[RegKind]) {
    let kinds: Option<Vec<(u16, ExitTag)>> = (pl.write_only.iter().enumerate())
        .filter(|(_, w)| **w)
        .map(|(r, _)| {
            let k = crate::jit_backend::trace::kinds::exit_tag_of(&tail[r]);
            luna_core::__private::kept_past_back_edge(k).then_some((r as u16, k))
        })
        .collect();
    lw.write_only_kinds = match (std::mem::replace(&mut lw.write_only_kinds, Err(())), kinds) {
        (Ok(had), Some(k)) if had.is_empty() || had == k => Ok(k),
        _ => Err(()),
    };
}
