use super::*;

/// A `string.sub` fold at its call: on a string and numbers the trace
/// knows as such; anything else is not compiled. A float position equal
/// to an integer is that integer in every dialect; any other float (which
/// 5.1 / 5.2 round and 5.3+ reject) leaves the trace at the fold's start,
/// whose argument set-up only writes the call's registers.
pub(super) fn emit_str_sub_fold<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    fold: &TraceMathFold,
) -> Option<()> {
    let RuntimeHelpers { str_sub_id, .. } = lw.h.rt;
    let Plan { record, .. } = *pl;
    let off = oc.off;
    let regs: &[Variable] = oc.regs;
    let kind = |r: u32| k_op(&lw.current_kinds, off as u32 + r);
    let a = fold.dst_reg;
    let number = |k| matches!(k, RegKind::Int | RegKind::Float);
    if kind(a + 1) != RegKind::Str
        || !number(kind(a + 2))
        || fold.nargs == 3 && !number(kind(a + 3))
    {
        return None;
    }
    let s = lw.bcx.use_var(regs[a as usize + 1]);
    let pos = |lw: &mut Lower<E>, r: u32| {
        let v = lw.bcx.use_var(regs[r as usize]);
        if k_op(&lw.current_kinds, off as u32 + r) != RegKind::Float {
            return v;
        }
        let (n, exact) = float_exact_int(&mut lw.bcx, v);
        guard!(lw, pl, exact, oc.i, record.ops[fold.start_idx].pc);
        n
    };
    let from = pos(lw, a + 2);
    let to = if fold.nargs == 3 {
        pos(lw, a + 3)
    } else {
        lw.bcx.ins().iconst(types::I64, -1)
    };
    let sub_ref = lw.bcx.import_func(str_sub_id);
    let call = lw.bcx.ins().call(sub_ref, &[s, from, to]);
    let r = lw.bcx.inst_results(call)[0];
    lw.bcx.def_var(regs[a as usize], r);
    lw.current_kinds[off + a as usize] = RegKind::Str;
    Some(())
}

/// A two-argument `math.min` / `math.max` fold at its call.
pub(super) fn emit_minmax_fold<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    fold: &TraceMathFold,
) -> Option<()> {
    let Plan {
        record, float_only, ..
    } = *pl;
    let OpCx { i, off, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    // 2-arg min/max. From 5.3 PUC's `math.min(a, b)`
    // returns one of its operands as it is, so the
    // lowering follows the recorded operand kinds:
    //
    //   5.1 / 5.2    → `fcmp` + `select`, as floats
    //   Int  / Int   → cranelift `smin` / `smax`
    //   Float/ Float → `fcmp` + `select`
    //   otherwise    → not compiled
    let k1 = k_op(&lw.current_kinds, off as u32 + fold.arg1_reg);
    let k2 = k_op(&lw.current_kinds, off as u32 + fold.arg2_reg);
    // `math.max` returns whichever argument wins,
    // unconverted (5.3+), so an Int/Float pair has no
    // static result kind: such a trace is not
    // compiled.
    // Anything but two numbers of one kind (strings
    // compare too, from 5.3) is not compiled either.
    let result_kind = match (k1, k2) {
        // 5.1 / 5.2 convert every argument to a
        // float (`luaL_checknumber`) and return
        // that float, whatever the argument kinds
        (RegKind::Int | RegKind::Float, RegKind::Int | RegKind::Float) if float_only => {
            RegKind::Float
        }
        (RegKind::Float, RegKind::Float) => RegKind::Float,
        (RegKind::Int, RegKind::Int) => RegKind::Int,
        (RegKind::Int, RegKind::Float) | (RegKind::Float, RegKind::Int) => {
            // The winner keeps its kind, which is
            // known only at run time. The trace
            // continues when the first argument wins
            // (PUC keeps it unless the second is
            // strictly better, compared exactly) and
            // otherwise leaves for the interpreter
            // at the fold's GetTabUp: the folded
            // GetTabUp / GetField never filled R[A],
            // and the argument set-up in between
            // only writes the call's argument slots,
            // so running it again is harmless.
            let a1 = lw.bcx.use_var(regs[fold.arg1_reg as usize]);
            let a2 = lw.bcx.use_var(regs[fold.arg2_reg as usize]);
            let f1 = lw.bcx.ins().bitcast(types::F64, MemFlagsData::new(), a1);
            let f2 = lw.bcx.ins().bitcast(types::F64, MemFlagsData::new(), a2);
            // max: second wins iff a1 < a2; min: iff a2 < a1.
            let second_wins = match (fold.kind, k1) {
                (FoldKind::Max2, RegKind::Int) => emit_lt_int_float(&mut lw.bcx, a1, f2),
                (FoldKind::Max2, _) => emit_lt_float_int(&mut lw.bcx, f1, a2),
                (FoldKind::Min2, RegKind::Int) => emit_lt_float_int(&mut lw.bcx, f2, a1),
                (FoldKind::Min2, _) => emit_lt_int_float(&mut lw.bcx, a2, f1),
                (FoldKind::Libm1 | FoldKind::StrSub | FoldKind::Fmod2, _) => unreachable!(),
            };
            let first_wins = lw.bcx.ins().bxor_imm_u(second_wins, 1);
            guard!(lw, pl, first_wins, i, record.ops[fold.start_idx].pc);
            lw.bcx.def_var(regs[fold.dst_reg as usize], a1);
            lw.current_kinds[off + fold.dst_reg as usize] = k1;
            return Some(());
        }
        _ => return None,
    };
    if matches!(result_kind, RegKind::Float) {
        let a1 = use_var_as_f64(&mut lw.bcx, regs, fold.arg1_reg, k1);
        let a2 = use_var_as_f64(&mut lw.bcx, regs, fold.arg2_reg, k2);
        // PUC keeps the first argument unless the
        // second compares strictly better — not
        // IEEE fmin/fmax, which differ on NaN and
        // on -0.0 vs 0.0.
        let second_wins = match fold.kind {
            FoldKind::Min2 => lw.bcx.ins().fcmp(FloatCC::LessThan, a2, a1),
            FoldKind::Max2 => lw.bcx.ins().fcmp(FloatCC::LessThan, a1, a2),
            FoldKind::Libm1 | FoldKind::StrSub | FoldKind::Fmod2 => unreachable!(),
        };
        // select on the bits: the baseline code generator selects
        // integers only
        let b2 = lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), a2);
        let b1 = lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), a1);
        let r = lw.bcx.ins().select(second_wins, b2, b1);
        lw.bcx.def_var(regs[fold.dst_reg as usize], r);
        lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Float;
    } else {
        // Int / Int — both operands are i64
        // payloads holding Int values. Use
        // signed integer min/max so the result
        // stays Int-tagged.
        let a1 = lw.bcx.use_var(regs[fold.arg1_reg as usize]);
        let a2 = lw.bcx.use_var(regs[fold.arg2_reg as usize]);
        let r = match fold.kind {
            FoldKind::Min2 => lw.bcx.ins().smin(a1, a2),
            FoldKind::Max2 => lw.bcx.ins().smax(a1, a2),
            FoldKind::Libm1 | FoldKind::StrSub | FoldKind::Fmod2 => unreachable!(),
        };
        lw.bcx.def_var(regs[fold.dst_reg as usize], r);
        lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Int;
    }
    Some(())
}
