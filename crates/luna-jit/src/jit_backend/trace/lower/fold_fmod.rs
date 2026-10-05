use super::*;

/// A two-argument `math.fmod` fold at its call, as the library does it:
/// from 5.3 two integers give C's truncating `%` (a divisor of -1 gives
/// 0; a zero divisor is the library's error, raised by the interpreter
/// after the trace leaves at the fold's start, whose argument set-up only
/// writes the call's registers); otherwise both operands are floats and
/// the result is the interpreter's C `fmod` (`luna_jit_fmod`, which picks
/// the NaN PUC's build picks when both are NaN). Anything but two numbers
/// the trace knows as such is not compiled.
pub(super) fn emit_fmod_fold<E: Emit>(
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
    let k1 = k_op(&lw.current_kinds, off as u32 + fold.arg1_reg);
    let k2 = k_op(&lw.current_kinds, off as u32 + fold.arg2_reg);
    let number = |k| matches!(k, RegKind::Int | RegKind::Float);
    if !number(k1) || !number(k2) {
        return None;
    }
    let dst = regs[fold.dst_reg as usize];
    if !float_only && k1 == RegKind::Int && k2 == RegKind::Int {
        let x = lw.bcx.use_var(regs[fold.arg1_reg as usize]);
        let d = lw.bcx.use_var(regs[fold.arg2_reg as usize]);
        let nonzero = lw.bcx.ins().icmp_imm_s(IntCC::NotEqual, d, 0);
        guard!(lw, pl, nonzero, i, record.ops[fold.start_idx].pc);
        // dividing by 1 instead of -1 keeps mininteger / -1 from trapping
        let minus_one = lw.bcx.ins().icmp_imm_s(IntCC::Equal, d, -1);
        let one = lw.bcx.ins().iconst(types::I64, 1);
        let dd = lw.bcx.ins().select(minus_one, one, d);
        let q = lw.bcx.ins().sdiv(x, dd);
        let p = lw.bcx.ins().imul(q, dd);
        let r = lw.bcx.ins().isub(x, p);
        let zero = lw.bcx.ins().iconst(types::I64, 0);
        let r = lw.bcx.ins().select(minus_one, zero, r);
        lw.bcx.def_var(dst, r);
        lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Int;
        return Some(());
    }
    let a = use_var_as_f64(&mut lw.bcx, regs, fold.arg1_reg, k1);
    let b = use_var_as_f64(&mut lw.bcx, regs, fold.arg2_reg, k2);
    let mut sig = lw.bcx.make_signature();
    sig.params.push(AbiParam::new(types::F64));
    sig.params.push(AbiParam::new(types::F64));
    sig.returns.push(AbiParam::new(types::F64));
    let id = lw
        .bcx
        .declare_function("luna_jit_fmod", Linkage::Import, &sig)
        .ok()?;
    let f = lw.bcx.import_func(id);
    let call = lw.bcx.ins().call(f, &[a, b]);
    let m = lw.bcx.inst_results(call)[0];
    def_var_f64(&mut lw.bcx, dst, m);
    lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Float;
    Some(())
}
