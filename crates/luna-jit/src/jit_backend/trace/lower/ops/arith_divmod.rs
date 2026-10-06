use super::*;

/// Float `//` and `%` (an integer operand converted first): `//` floors
/// the quotient; `%` is each dialect's `luai_nummod`, as the interpreter's
/// `float_mod` does it: 5.1 / 5.2 `a - floor(a/b)*b`, 5.3 `fmod` corrected
/// when `m*b < 0`, 5.4+ `fmod` corrected when the signs of `m` and `b`
/// differ.
pub(super) fn emit_float_divmod<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    kb: RegKind,
    kc: RegKind,
) -> Option<()> {
    let Plan {
        opts, float_only, ..
    } = *pl;
    let OpCx { off, ins, op, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    let a = use_var_as_f64(&mut lw.bcx, regs, ins.b(), kb);
    let b = use_var_as_f64(&mut lw.bcx, regs, ins.c(), kc);
    let r = if op == Op::IDiv {
        let q = lw.bcx.ins().fdiv(a, b);
        lw.bcx.ins().floor(q)
    } else if float_only {
        let q = lw.bcx.ins().fdiv(a, b);
        let fl = lw.bcx.ins().floor(q);
        let p = lw.bcx.ins().fmul(fl, b);
        lw.bcx.ins().fsub(a, p)
    } else {
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
        let zero = lw.bcx.ins().f64const(0.0);
        let fix = if opts.pre53 {
            let p = lw.bcx.ins().fmul(m, b);
            lw.bcx.ins().fcmp(FloatCC::LessThan, p, zero)
        } else {
            let m_pos = lw.bcx.ins().fcmp(FloatCC::GreaterThan, m, zero);
            let b_neg = lw.bcx.ins().fcmp(FloatCC::LessThan, b, zero);
            let m_neg = lw.bcx.ins().fcmp(FloatCC::LessThan, m, zero);
            let b_pos = lw.bcx.ins().fcmp(FloatCC::GreaterThan, b, zero);
            let x = lw.bcx.ins().band(m_pos, b_neg);
            let y = lw.bcx.ins().band(m_neg, b_pos);
            lw.bcx.ins().bor(x, y)
        };
        // select on the bits: the baseline code generator selects
        // integers only
        let fixed = lw.bcx.ins().fadd(m, b);
        let fixed = lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), fixed);
        let m = lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), m);
        let bits = lw.bcx.ins().select(fix, fixed, m);
        lw.bcx.def_var(regs[ins.a() as usize], bits);
        lw.current_kinds[off + ins.a() as usize] = RegKind::Float;
        return Some(());
    };
    def_var_f64(&mut lw.bcx, regs[ins.a() as usize], r);
    lw.current_kinds[off + ins.a() as usize] = RegKind::Float;
    Some(())
}
