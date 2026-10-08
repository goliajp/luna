use super::*;

pub(super) fn emit_for_prep(
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) {
    let EmitFacts {
        c: ChunkIn { pre53, .. },
        scan,
        reg_kinds,
        regs,
        pc_to_block,
        ..
    } = f;
    let ChunkScan { for_loops, .. } = scan;
    let EmitState {
        current_kinds,
        terminated,
        ..
    } = st;
    match ins.op() {
        Op::ForPrep | Op::ForPrep55 => {
            let r = ForRegs::of(ins);
            // 5.5 loops count down whatever the dialect
            let pre53 = pre53 && ins.op() == Op::ForPrep;
            let &(_, loop_pc, step_imm) = for_loops
                .iter()
                .find(|&&(p, _, _)| p == pc)
                .expect("scanner recorded this ForPrep");
            let a = ins.a() as usize;
            let is_float = matches!(a_kind(reg_kinds, ins.a()), RegKind::Float);
            let step_i = bcx.ins().iconst(types::I64, step_imm);

            match (pre53, is_float) {
                (true, false) => {
                    // pre-5.3 Int form. R[A] = init - step
                    // (so ForLoop's first add lands on init), copy
                    // limit + step over, unconditional jump to the
                    // ForLoop block. R[A+3] left alone — pre53
                    // ForLoop writes it on continue.
                    let init = bcx.use_var(regs[a]);
                    let limit = bcx.use_var(regs[a + 1]);
                    let pre = bcx.ins().isub(init, step_i);
                    aligned_def(bcx, regs, reg_kinds, a, pre);
                    aligned_def(bcx, regs, reg_kinds, a + 1, limit);
                    aligned_def(bcx, regs, reg_kinds, a + 2, step_i);
                    current_kinds[a] = RegKind::Int;
                    current_kinds[a + 1] = RegKind::Int;
                    current_kinds[a + 2] = RegKind::Int;
                    let loop_blk = pc_to_block[loop_pc].expect("ForLoop BB");
                    bcx.ins().jump(loop_blk, &[]);
                    *terminated = true;
                }
                (false, false) => {
                    // 5.4+ Int count form.
                    let init = bcx.use_var(regs[a]);
                    let limit = bcx.use_var(regs[a + 1]);

                    let empty = if step_imm > 0 {
                        bcx.ins().icmp(IntCC::SignedGreaterThan, init, limit)
                    } else {
                        bcx.ins().icmp(IntCC::SignedLessThan, init, limit)
                    };

                    // count = (limit - init) / step (positive-step)
                    //       = (init - limit) / -step (negative-step)
                    // Both are unsigned (PUC `lua_Unsigned`): the span
                    // of a loop over most of the integer range does not
                    // fit an i64.
                    let span = if step_imm > 0 {
                        bcx.ins().isub(limit, init)
                    } else {
                        bcx.ins().isub(init, limit)
                    };
                    // `math.mininteger` as a step: its magnitude is
                    // 2^63, which only the unsigned division sees right
                    let abs_step = bcx.ins().iconst(types::I64, step_imm.unsigned_abs() as i64);
                    let count = bcx.ins().udiv(span, abs_step);

                    aligned_def(bcx, regs, reg_kinds, r.x, count);
                    aligned_def(bcx, regs, reg_kinds, r.step, step_i);
                    aligned_def(bcx, regs, reg_kinds, r.idx, init);
                    aligned_def(bcx, regs, reg_kinds, r.var, init);
                    for reg in [r.idx, r.x, r.step, r.var] {
                        current_kinds[reg] = RegKind::Int;
                    }

                    let body_blk = pc_to_block[pc + 1].expect("body BB start");
                    let exit_blk = pc_to_block[loop_pc + 1].expect("exit BB start");
                    bcx.ins().brif(empty, exit_blk, &[], body_blk, &[]);
                    *terminated = true;
                }
                (true, true) => {
                    // pre-5.3 Float form. R[A] = init - step,
                    // R[A+1] = limit, R[A+2] = step, unconditional
                    // jump to the ForLoop block. step_imm is the
                    // (Int) immediate the bytecode put in R[A+2];
                    // we promote it to f64 for arith and write its
                    // Int bit-pattern to R[A+2]'s declared Int slot.
                    let init = bcx.use_var(regs[a]);
                    let limit = bcx.use_var(regs[a + 1]);
                    let step_f = bcx.ins().f64const(step_imm as f64);
                    let pre = bcx.ins().fsub(init, step_f);
                    aligned_def(bcx, regs, reg_kinds, a, pre);
                    aligned_def(bcx, regs, reg_kinds, a + 1, limit);
                    aligned_def(bcx, regs, reg_kinds, a + 2, step_i);
                    current_kinds[a] = RegKind::Float;
                    current_kinds[a + 1] = RegKind::Float;
                    current_kinds[a + 2] = RegKind::Int;
                    let loop_blk = pc_to_block[loop_pc].expect("ForLoop BB");
                    bcx.ins().jump(loop_blk, &[]);
                    *terminated = true;
                }
                (false, true) => {
                    // 5.4+ Float form. Mirrors interp's
                    // post53 Float branch in `for_prep`: empty test
                    // `init > limit` (positive step) / `init < limit`
                    // (negative step), and on continue write R[A] =
                    // init, R[A+1] = limit, R[A+2] = step, R[A+3] =
                    // init, fall through to body. No count form for
                    // Float — R[A+1] keeps the limit, not a
                    // remaining-count.
                    let init = bcx.use_var(regs[a]);
                    let limit = bcx.use_var(regs[a + 1]);
                    let step_f = bcx.ins().f64const(step_imm as f64);

                    let empty = if step_imm > 0 {
                        bcx.ins().fcmp(FloatCC::GreaterThan, init, limit)
                    } else {
                        bcx.ins().fcmp(FloatCC::LessThan, init, limit)
                    };

                    let set_blk = bcx.create_block();
                    let exit_blk = pc_to_block[loop_pc + 1].expect("exit BB start");
                    bcx.ins().brif(empty, exit_blk, &[], set_blk, &[]);
                    *terminated = true;

                    bcx.switch_to_block(set_blk);
                    bcx.seal_block(set_blk);
                    // 5.5 keeps a float step in the limit's register
                    let (step, step_kind) = if r.var == r.idx {
                        (step_f, RegKind::Float)
                    } else {
                        (step_i, RegKind::Int)
                    };
                    aligned_def(bcx, regs, reg_kinds, r.x, limit);
                    aligned_def(bcx, regs, reg_kinds, r.step, step);
                    aligned_def(bcx, regs, reg_kinds, r.idx, init);
                    aligned_def(bcx, regs, reg_kinds, r.var, init);
                    current_kinds[r.idx] = RegKind::Float;
                    current_kinds[r.x] = RegKind::Float;
                    current_kinds[r.step] = step_kind;
                    current_kinds[r.var] = RegKind::Float;
                    let body_blk = pc_to_block[pc + 1].expect("body BB start");
                    bcx.ins().jump(body_blk, &[]);
                }
            }
        }
        _ => unreachable!("dispatched by op"),
    }
}

pub(super) fn emit_for_loop(
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) {
    let EmitFacts {
        c: ChunkIn { pre53, .. },
        scan,
        reg_kinds,
        regs,
        pc_to_block,
        ..
    } = f;
    let ChunkScan { for_loops, .. } = scan;
    let EmitState {
        current_kinds,
        terminated,
        ..
    } = st;
    match ins.op() {
        Op::ForLoop | Op::ForLoop55 => {
            let r = ForRegs::of(ins);
            let pre53 = pre53 && ins.op() == Op::ForLoop;
            let prep_pc = for_loops
                .iter()
                .find(|&&(_, lp, _)| lp == pc)
                .map(|&(p, _, _)| p)
                .expect("scanner paired this ForLoop");
            let &(_, _, step_imm) = for_loops
                .iter()
                .find(|&&(p, _, _)| p == prep_pc)
                .expect("step_const recorded");
            let a = ins.a() as usize;
            let is_float = matches!(a_kind(reg_kinds, ins.a()), RegKind::Float);

            if is_float {
                // Float ForLoop. Same shape for pre53 and
                // post53 (Float Loop never used the count form).
                // next = R[A] + step; cont = next ≤ limit (positive)
                // / next ≥ limit (negative). On continue → R[A] =
                // next, R[A+3] = next, back-jump to body.
                let cur = bcx.use_var(regs[r.idx]);
                let step_f = bcx.ins().f64const(step_imm as f64);
                let next = bcx.ins().fadd(cur, step_f);
                let limit = bcx.use_var(regs[r.x]);
                let cont = if step_imm > 0 {
                    bcx.ins().fcmp(FloatCC::LessThanOrEqual, next, limit)
                } else {
                    bcx.ins().fcmp(FloatCC::GreaterThanOrEqual, next, limit)
                };
                let continue_blk = bcx.create_block();
                let body_blk = pc_to_block[prep_pc + 1].expect("body BB");
                let exit_blk = pc_to_block[pc + 1].expect("exit BB");
                bcx.ins().brif(cont, continue_blk, &[], exit_blk, &[]);
                *terminated = true;

                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
                aligned_def(bcx, regs, reg_kinds, r.idx, next);
                aligned_def(bcx, regs, reg_kinds, r.var, next);
                current_kinds[r.idx] = RegKind::Float;
                current_kinds[r.var] = RegKind::Float;
                bcx.ins().jump(body_blk, &[]);
            } else if pre53 {
                // pre-5.3 Int form. R[A] += step; check vs
                // R[A+1] = limit; continue → write R[A+3] = R[A]
                // + backward jump.
                let cur = bcx.use_var(regs[a]);
                let step_v = bcx.ins().iconst(types::I64, step_imm);
                let next = bcx.ins().iadd(cur, step_v);
                let limit = bcx.use_var(regs[a + 1]);
                let cont = if step_imm > 0 {
                    bcx.ins().icmp(IntCC::SignedLessThanOrEqual, next, limit)
                } else {
                    bcx.ins().icmp(IntCC::SignedGreaterThanOrEqual, next, limit)
                };
                let continue_blk = bcx.create_block();
                let body_blk = pc_to_block[prep_pc + 1].expect("body BB");
                let exit_blk = pc_to_block[pc + 1].expect("exit BB");
                bcx.ins().brif(cont, continue_blk, &[], exit_blk, &[]);
                *terminated = true;

                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
                aligned_def(bcx, regs, reg_kinds, a, next);
                aligned_def(bcx, regs, reg_kinds, a + 3, next);
                current_kinds[a] = RegKind::Int;
                current_kinds[a + 3] = RegKind::Int;
                bcx.ins().jump(body_blk, &[]);
            } else {
                // 5.4+ Int count form.
                let count = bcx.use_var(regs[r.x]);
                let zero_i = bcx.ins().iconst(types::I64, 0);
                // unsigned count (see ForPrep)
                let cont = bcx.ins().icmp(IntCC::NotEqual, count, zero_i);

                let continue_blk = bcx.create_block();
                let body_blk = pc_to_block[prep_pc + 1].expect("body BB");
                let exit_blk = pc_to_block[pc + 1].expect("exit BB");
                bcx.ins().brif(cont, continue_blk, &[], exit_blk, &[]);
                *terminated = true;

                bcx.switch_to_block(continue_blk);
                bcx.seal_block(continue_blk);
                let cur = bcx.use_var(regs[r.idx]);
                let step_v = bcx.ins().iconst(types::I64, step_imm);
                let next = bcx.ins().iadd(cur, step_v);
                let one = bcx.ins().iconst(types::I64, 1);
                let new_count = bcx.ins().isub(count, one);
                aligned_def(bcx, regs, reg_kinds, r.idx, next);
                aligned_def(bcx, regs, reg_kinds, r.x, new_count);
                aligned_def(bcx, regs, reg_kinds, r.var, next);
                current_kinds[r.idx] = RegKind::Int;
                current_kinds[r.x] = RegKind::Int;
                current_kinds[r.var] = RegKind::Int;
                bcx.ins().jump(body_blk, &[]);
            }
        }
        _ => unreachable!("dispatched by op"),
    }
}
