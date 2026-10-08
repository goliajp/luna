//! Fast-loop arms for arithmetic, bitwise and unary operators.
//! The generator defines one macro per opcode inside `run_fast`; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_arith_arms {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        macro_rules! op_add {
            () => {{
                arith_rr!(Add, int(a, b) => Some(if DBL { dbl::add(a, b) } else { Value::Int(a.wrapping_add(b)) }), float(a, b) => Some(Value::Float(a + b)))
            }};
        }
        macro_rules! op_sub {
            () => {{
                arith_rr!(Sub, int(a, b) => Some(if DBL { dbl::sub(a, b) } else { Value::Int(a.wrapping_sub(b)) }), float(a, b) => Some(Value::Float(a - b)))
            }};
        }
        macro_rules! op_mul {
            () => {{
                arith_rr!(Mul, int(a, b) => Some(if DBL { dbl::mul(a, b) } else { Value::Int(a.wrapping_mul(b)) }), float(a, b) => Some(Value::Float(a * b)))
            }};
        }
        // a zero divisor takes the slow path for its error
        macro_rules! op_mod {
            () => {{
                arith_rr!(Mod, int(a, b) => if DBL { Some(dbl::rem(a, b)) } else { int_mod_or_zero_div(a, b).map(Value::Int) }, float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_i_div {
            () => {{
                arith_rr!(IDiv, int(a, b) => (b != 0).then(|| Value::Int(int_idiv(a, b))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_div {
            () => {{
                arith_rr!(Div, int(a, b) => Some(Value::Float(a as f64 / b as f64)), float(a, b) => Some(Value::Float(a / b)))
            }};
        }
        macro_rules! op_b_and {
            () => {{
                arith_rr!(BAnd, int(a, b) => Some(Value::Int(a & b)), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_b_or {
            () => {{
                arith_rr!(BOr, int(a, b) => Some(Value::Int(a | b)), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_b_xor {
            () => {{
                arith_rr!(BXor, int(a, b) => Some(Value::Int(a ^ b)), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_shl {
            () => {{
                arith_rr!(Shl, int(a, b) => Some(Value::Int(shift_left(a, b))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_shr {
            () => {{
                arith_rr!(Shr, int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_add_i {
            () => {{
                arith_ri!(Add, int(a, b) => Some(if DBL { dbl::add(a, b) } else { Value::Int(a.wrapping_add(b)) }), float(a, b) => Some(Value::Float(a + b)))
            }};
        }
        macro_rules! op_sub_i {
            () => {{
                arith_ri!(Sub, int(a, b) => Some(if DBL { dbl::sub(a, b) } else { Value::Int(a.wrapping_sub(b)) }), float(a, b) => Some(Value::Float(a + (0.0 - b))))
            }};
        }
        macro_rules! op_add_k {
            () => {{
                arith_rk!(Add, int(a, b) => Some(if DBL { dbl::add(a, b) } else { Value::Int(a.wrapping_add(b)) }), float(a, b) => Some(Value::Float(a + b)))
            }};
        }
        macro_rules! op_sub_k {
            () => {{
                arith_rk_ordered!(Sub, int(a, b) => Some(if DBL { dbl::sub(a, b) } else { Value::Int(a.wrapping_sub(b)) }), float(a, b) => Some(Value::Float(a - b)))
            }};
        }
        macro_rules! op_mul_k {
            () => {{
                arith_rk!(Mul, int(a, b) => Some(if DBL { dbl::mul(a, b) } else { Value::Int(a.wrapping_mul(b)) }), float(a, b) => Some(Value::Float(a * b)))
            }};
        }
        // a zero divisor takes the slow path for its error
        macro_rules! op_mod_k {
            () => {{
                arith_rk_ordered!(Mod, int(a, b) => if DBL { Some(dbl::rem(a, b)) } else { int_mod_or_zero_div(a, b).map(Value::Int) }, float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_i_div_k {
            () => {{
                arith_rk_ordered!(IDiv, int(a, b) => (b != 0).then(|| Value::Int(int_idiv(a, b))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_div_k {
            () => {{
                arith_rk_ordered!(Div, int(a, b) => Some(Value::Float(a as f64 / b as f64)), float(a, b) => Some(Value::Float(a / b)))
            }};
        }
        macro_rules! op_pow_k {
            () => {{
                arith_rk_ordered!(Pow, int(a, b) => Some(Value::Float(num_pow($vm.version() >= LuaVersion::Lua54, a as f64, b as f64))), float(a, b) => Some(Value::Float(num_pow($vm.version() >= LuaVersion::Lua54, a, b))))
            }};
        }
        macro_rules! op_b_and_k {
            () => {{
                arith_rk!(BAnd, int(a, b) => Some(Value::Int(a & b)), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_b_or_k {
            () => {{
                arith_rk!(BOr, int(a, b) => Some(Value::Int(a | b)), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_b_xor_k {
            () => {{
                arith_rk!(BXor, int(a, b) => Some(Value::Int(a ^ b)), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_shr_i {
            () => {{
                arith_ri!(Shr, int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_shl_i {
            () => {{
                if $inst.k() {
                    // `sC << R[B]` (5.4+ `SHLI`)
                    let x = reg!($inst.b());
                    if let Value::Int(n) = x {
                        set_reg!($inst.a(), Value::Int(shift_left($inst.sc() as i64, n)));
                        next!()
                    }
                    save!();
                    $vm.arith_slow($inst.a(), base!(), ArithOp::Shl, Value::Int($inst.sc() as i64), x)?;
                    resume_same!()
                }
                arith_ri!(Shl, int(a, b) => Some(Value::Int(shift_left(a, b))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_shl_k {
            () => {{
                arith_rk_ordered!(Shl, int(a, b) => Some(Value::Int(shift_left(a, b))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_shr_k {
            () => {{
                arith_rk_ordered!(Shr, int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))), float(a, b) => { let _ = (a, b); None })
            }};
        }
        macro_rules! op_unm {
            () => {{
                let v = reg!($inst.b());
                match $vm.unary_operand(v) {
                    Some(Num::Int(i)) => {
                        set_reg!(
                            $inst.a(),
                            if DBL {
                                dbl::neg(i)
                            } else {
                                Value::Int(i.wrapping_neg())
                            }
                        );
                        next!()
                    }
                    Some(Num::Float(f)) => {
                        set_reg!($inst.a(), Value::Float(-f));
                        next!()
                    }
                    None => {
                        save!();
                        let mm = $vm.get_mm(v, Mm::Unm);
                        if mm.is_nil() {
                            return Err($vm.type_err("perform arithmetic on", v));
                        }
                        let dst = base!() + $inst.a();
                        $vm.begin_meta_call(mm, &[v, v], MetaAction::Store { dst })?;
                        resume_same!()
                    }
                }
            }};
        }
        macro_rules! op_b_not {
            () => {{
                let v = reg!($inst.b());
                match $vm.arith_operand()(v) {
                    Some(n) => {
                        let Some(i) = int_of(n) else {
                            save!();
                            return Err($vm.no_int_rep_err());
                        };
                        set_reg!($inst.a(), Value::Int(!i));
                        next!()
                    }
                    None => {
                        save!();
                        let mm = $vm.get_mm(v, Mm::BNot);
                        if mm.is_nil() {
                            return Err($vm.type_err("perform bitwise operation on", v));
                        }
                        let dst = base!() + $inst.a();
                        $vm.begin_meta_call(mm, &[v, v], MetaAction::Store { dst })?;
                        resume_same!()
                    }
                }
            }};
        }
        macro_rules! op_not {
            () => {{
                let t = reg_truthy!($inst.b());
                set_reg!($inst.a(), Value::Bool(!t));
                next!()
            }};
        }
        macro_rules! op_len {
            () => {{
                let v = reg!($inst.b());
                // no `__len` to look for: a string, or a table without
                // a metatable
                match v {
                    Value::Str(s) => {
                        set_reg!($inst.a(), Value::Int(s.len() as i64));
                        next!()
                    }
                    Value::Table(t) if t.metatable().is_none() => {
                        set_reg!($inst.a(), Value::Int(t.len()));
                        next!()
                    }
                    _ => {}
                }
                save!();
                match $vm.len_step(v)? {
                    MmOut::Done(r) => $vm.set_r(base!(), $inst.a(), r),
                    MmOut::Mm { func, recv } => {
                        let dst = base!() + $inst.a();
                        $vm.begin_meta_call(
                            func,
                            &[recv, recv],
                            MetaAction::Store { dst },
                        )?;
                    }
                    MmOut::CompareSynth { .. } => {
                        unreachable!("CompareSynth from len_step")
                    }
                }
            }};
        }
    };
}
pub(super) use fast_arith_arms;
