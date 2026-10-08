use super::emit_for::shadow_index;
use super::*;

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
                let shadow = shadow_index(f, prep_pc);
                let cur = bcx.use_var(shadow.unwrap_or(regs[r.idx]));
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
                if let Some(v) = shadow {
                    bcx.def_var(v, next);
                }
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
                let shadow = shadow_index(f, prep_pc);
                let cur = bcx.use_var(shadow.unwrap_or(regs[r.idx]));
                let step_v = bcx.ins().iconst(types::I64, step_imm);
                let next = bcx.ins().iadd(cur, step_v);
                if let Some(v) = shadow {
                    bcx.def_var(v, next);
                }
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
