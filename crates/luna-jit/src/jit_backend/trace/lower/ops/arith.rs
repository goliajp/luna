use super::*;

/// Arithmetic that may produce a float.
pub(super) fn emit_float_arith_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan { opts, .. } = *pl;
    let OpCx { off, ins, op, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => {
            let kb = oc.kind(&lw.current_kinds, ins.b());
            let kc = oc.kind(&lw.current_kinds, ins.c());
            // A string operand is coerced (or has `__add` & co. in its
            // metatable); only numbers are lowered, since the payload of
            // anything else is a pointer.
            let number = |k| matches!(k, RegKind::Int | RegKind::Float);
            if !number(kb) || !number(kc) {
                return None;
            }
            // Op::Pow always returns Float in Lua 5.4+ (matches
            // `pow(f64, f64) -> f64`); coerce Int operands to
            // Float via fcvt_from_sint.
            if matches!(op, Op::Pow) {
                let lhs = match kb {
                    RegKind::Float => use_var_f64(&mut lw.bcx, regs, ins.b()),
                    _ => {
                        let raw = lw.bcx.use_var(regs[ins.b() as usize]);
                        lw.bcx.ins().fcvt_from_sint(types::F64, raw)
                    }
                };
                let rhs = match kc {
                    RegKind::Float => use_var_f64(&mut lw.bcx, regs, ins.c()),
                    _ => {
                        let raw = lw.bcx.use_var(regs[ins.c() as usize]);
                        lw.bcx.ins().fcvt_from_sint(types::F64, raw)
                    }
                };
                let mut pow_sig = lw.bcx.make_signature();
                pow_sig.params.push(AbiParam::new(types::F64));
                pow_sig.params.push(AbiParam::new(types::F64));
                pow_sig.returns.push(AbiParam::new(types::F64));
                let pow_id = lw
                    .bcx
                    .declare_function("pow", Linkage::Import, &pow_sig)
                    .ok()?;
                let pow_ref = lw.bcx.import_func(pow_id);
                let call = lw.bcx.ins().call(pow_ref, &[lhs, rhs]);
                let mut r = lw.bcx.inst_results(call)[0];
                // 5.4+ `luai_numpow` squares by multiplying, which can
                // differ from `pow` in the last bit
                if !opts.pre53 {
                    let two = lw.bcx.ins().f64const(2.0);
                    let is_two = lw.bcx.ins().fcmp(FloatCC::Equal, rhs, two);
                    let sq = lw.bcx.ins().fmul(lhs, lhs);
                    r = lw.bcx.ins().select(is_two, sq, r);
                }
                def_var_f64(&mut lw.bcx, regs[ins.a() as usize], r);
                lw.current_kinds[off + ins.a() as usize] = RegKind::Float;
                return Some(());
            }
            // Float path when either operand is known-Float.
            // Both must be Float — mixed Int+Float would
            // semantically coerce to Float in Lua, but the
            // trace's kind tracker bails to avoid the
            // ambiguous emit.
            let float_path = matches!(kb, RegKind::Float) || matches!(kc, RegKind::Float);
            if float_path {
                if !matches!(kb, RegKind::Float) || !matches!(kc, RegKind::Float) {
                    return None;
                }
                let lhs = use_var_f64(&mut lw.bcx, regs, ins.b());
                let rhs = use_var_f64(&mut lw.bcx, regs, ins.c());
                let r = match op {
                    Op::Add => lw.bcx.ins().fadd(lhs, rhs),
                    Op::Sub => lw.bcx.ins().fsub(lhs, rhs),
                    Op::Mul => lw.bcx.ins().fmul(lhs, rhs),
                    Op::Div => lw.bcx.ins().fdiv(lhs, rhs),
                    _ => unreachable!(),
                };
                def_var_f64(&mut lw.bcx, regs[ins.a() as usize], r);
                lw.current_kinds[off + ins.a() as usize] = RegKind::Float;
            } else {
                // Op::Div on Int operands would still coerce
                // to Float in Lua 5.4+. Bail to be safe.
                if matches!(op, Op::Div) {
                    return None;
                }
                let lhs = lw.bcx.use_var(regs[ins.b() as usize]);
                let rhs = lw.bcx.use_var(regs[ins.c() as usize]);
                let r = match op {
                    Op::Add => lw.bcx.ins().iadd(lhs, rhs),
                    Op::Sub => lw.bcx.ins().isub(lhs, rhs),
                    Op::Mul => lw.bcx.ins().imul(lhs, rhs),
                    _ => unreachable!(),
                };
                lw.bcx.def_var(regs[ins.a() as usize], r);
                lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// Integer-division, modulo, bitwise ops and the unary ops.
pub(super) fn emit_int_arith_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let OpCx {
        i,
        rop,
        rc_const,
        off,
        ins,
        op,
        ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        // 3-reg Int ops. The cases the machine instruction gets
        // wrong for Lua — a zero divisor (Lua raises), a shift count
        // outside 0..=63 (Lua shifts the other way or gives 0) — leave
        // the trace at the op so the interpreter does them; a -1
        // divisor (the machine traps on minint) is done inline.
        Op::IDiv | Op::Mod | Op::BAnd | Op::BOr | Op::BXor | Op::Shl | Op::Shr => {
            // Lowered for two integers only: a float operand makes
            // `//` and `%` float ops and the bitwise ops convert or
            // raise; a string is coerced.
            let kb = oc.kind(&lw.current_kinds, ins.b());
            let kc = oc.kind(&lw.current_kinds, ins.c());
            if !matches!(kb, RegKind::Int) || !matches!(kc, RegKind::Int) {
                return None;
            }
            let lhs = lw.bcx.use_var(regs[ins.b() as usize]);
            let rhs = lw.bcx.use_var(regs[ins.c() as usize]);
            let r = match (op, rc_const) {
                // a constant divisor needs neither guard (and the
                // machine division by a constant is strength-reduced)
                (Op::IDiv | Op::Mod, Some(k)) if k != 0 && k != -1 => {
                    emit_floor_divmod_by(&mut lw.bcx, op, lhs, k)
                }
                // a constant shift count is a single machine shift
                (Op::Shl | Op::Shr, Some(k)) => {
                    let n = if op == Op::Shr { k.wrapping_neg() } else { k };
                    if n <= -64 || n >= 64 {
                        lw.bcx.ins().iconst(types::I64, 0)
                    } else if n >= 0 {
                        lw.bcx.ins().ishl_imm_u(lhs, n)
                    } else {
                        lw.bcx.ins().ushr_imm_u(lhs, -n)
                    }
                }
                _ => match op {
                    Op::IDiv | Op::Mod => {
                        // A zero divisor is the interpreter's error to
                        // raise: leave the trace at this op.
                        let zero = lw.bcx.ins().iconst(types::I64, 0);
                        let is_zero = lw.bcx.ins().icmp(IntCC::Equal, rhs, zero);
                        let cont_blk = lw.bcx.create_block();
                        let exit_blk = lw.bcx.create_block();
                        lw.bcx.ins().brif(is_zero, exit_blk, &[], cont_blk, &[]);
                        lw.bcx.switch_to_block(exit_blk);
                        lw.bcx.seal_block(exit_blk);
                        guard_exit(lw, pl, rop.pc, i);
                        lw.bcx.switch_to_block(cont_blk);
                        lw.bcx.seal_block(cont_blk);
                        emit_floor_divmod(&mut lw.bcx, op, lhs, rhs)
                    }
                    Op::BAnd => lw.bcx.ins().band(lhs, rhs),
                    Op::BOr => lw.bcx.ins().bor(lhs, rhs),
                    Op::BXor => lw.bcx.ins().bxor(lhs, rhs),
                    Op::Shl | Op::Shr => {
                        let wide = lw.bcx.ins().icmp_imm_u(IntCC::UnsignedGreaterThan, rhs, 63);
                        let cont_blk = lw.bcx.create_block();
                        let exit_blk = lw.bcx.create_block();
                        lw.bcx.ins().brif(wide, exit_blk, &[], cont_blk, &[]);
                        lw.bcx.switch_to_block(exit_blk);
                        lw.bcx.seal_block(exit_blk);
                        guard_exit(lw, pl, rop.pc, i);
                        lw.bcx.switch_to_block(cont_blk);
                        lw.bcx.seal_block(cont_blk);
                        if op == Op::Shl {
                            lw.bcx.ins().ishl(lhs, rhs)
                        } else {
                            lw.bcx.ins().ushr(lhs, rhs)
                        }
                    }
                    _ => unreachable!("whitelist gated above"),
                },
            };
            lw.bcx.def_var(regs[ins.a() as usize], r);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
        }
        Op::Unm | Op::BNot => {
            let kb = k_op(&lw.current_kinds, off as u32 + ins.b());
            if !matches!(kb, RegKind::Int)
                && !(matches!(op, Op::Unm) && matches!(kb, RegKind::Float))
            {
                return None;
            }
            if matches!(op, Op::Unm) && matches!(kb, RegKind::Float) {
                // Float negation.
                let src = use_var_f64(&mut lw.bcx, regs, ins.b());
                let r = lw.bcx.ins().fneg(src);
                def_var_f64(&mut lw.bcx, regs[ins.a() as usize], r);
                lw.current_kinds[off + ins.a() as usize] = RegKind::Float;
            } else {
                let src = lw.bcx.use_var(regs[ins.b() as usize]);
                let r = match op {
                    Op::Unm => lw.bcx.ins().ineg(src),
                    Op::BNot => lw.bcx.ins().bnot(src),
                    _ => unreachable!("whitelist gated above"),
                };
                lw.bcx.def_var(regs[ins.a() as usize], r);
                lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}
