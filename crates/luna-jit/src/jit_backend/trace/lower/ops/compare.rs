use super::*;

/// Comparison with a constant.
pub(super) fn emit_eqk_op<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    let OpCx { rop, off, ins, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        Op::EqK => {
            // `R[A] == const[B]` — Int and Float consts both
            // valid (pre-emit gated above). Emit icmp eq for
            // Int + Int, fcmp eq for Float + Float; a number
            // against the other number kind bails, against a
            // non-number is never equal.
            let bx = ins.b() as usize;
            let ka = k_op(&lw.current_kinds, off as u32 + ins.a());
            let cond = match oc.rop.proto.consts[bx] {
                luna_core::runtime::Value::Int(n) => {
                    if matches!(ka, RegKind::Float) {
                        return None;
                    }
                    match eq_lowering(ka, RegKind::Int) {
                        EqLowering::Unequal => lw.bcx.ins().iconst(types::I8, i64::from(!ins.k())),
                        lowering => {
                            if lowering == EqLowering::Unknown {
                                lw.dispatchable = false;
                                lw.dispatch_off_reason =
                                    lw.dispatch_off_reason.or(Some("cmp:unknown-kind"));
                            }
                            let lhs = lw.bcx.use_var(regs[ins.a() as usize]);
                            let rhs = lw.bcx.ins().iconst(types::I64, n);
                            let int_cc = if ins.k() {
                                IntCC::Equal
                            } else {
                                IntCC::NotEqual
                            };
                            lw.bcx.ins().icmp(int_cc, lhs, rhs)
                        }
                    }
                }
                luna_core::runtime::Value::Float(f) => {
                    if !matches!(ka, RegKind::Float) {
                        return None;
                    }
                    let lhs = use_var_f64(&mut lw.bcx, regs, ins.a());
                    let rhs = lw.bcx.ins().f64const(f);
                    let float_cc = if ins.k() {
                        FloatCC::Equal
                    } else {
                        FloatCC::NotEqual
                    };
                    lw.bcx.ins().fcmp(float_cc, lhs, rhs)
                }
                luna_core::runtime::Value::Str(k) => match eq_lowering(ka, RegKind::Str) {
                    EqLowering::Unequal => lw.bcx.ins().iconst(types::I8, i64::from(!ins.k())),
                    EqLowering::Unknown => return None,
                    _ => {
                        let lhs = lw.bcx.use_var(regs[ins.a() as usize]);
                        let rhs =
                            emit_str_key_arg(&mut lw.bcx, k, pl.opts.aot, &mut lw.defined_aot_data);
                        let int_cc = if ins.k() {
                            IntCC::Equal
                        } else {
                            IntCC::NotEqual
                        };
                        lw.bcx.ins().icmp(int_cc, lhs, rhs)
                    }
                },
                luna_core::runtime::Value::Bool(b) => match eq_lowering(ka, RegKind::Bool) {
                    EqLowering::Unequal => lw.bcx.ins().iconst(types::I8, i64::from(!ins.k())),
                    EqLowering::Unknown => return None,
                    _ => {
                        let lhs = lw.bcx.use_var(regs[ins.a() as usize]);
                        let int_cc = if ins.k() {
                            IntCC::Equal
                        } else {
                            IntCC::NotEqual
                        };
                        lw.bcx.ins().icmp_imm_u(int_cc, lhs, i64::from(b))
                    }
                },
                _ => unreachable!("pre-emit gates number, boolean and short string consts only"),
            };

            let continue_blk = lw.bcx.create_block();
            let side_exit_blk = lw.bcx.create_block();
            lw.bcx
                .ins()
                .brif(cond, continue_blk, &[], side_exit_blk, &[]);

            lw.bcx.switch_to_block(side_exit_blk);
            lw.bcx.seal_block(side_exit_blk);
            if alt_taken(lw, pl, oc.i, continue_blk) {
                lw.bcx.switch_to_block(continue_blk);
                lw.bcx.seal_block(continue_blk);
                return Some(());
            }
            let side_exit_pc = rop.pc + 2;
            // at depth>0, the side-exit must
            // materialise the inlined frames before the interp can
            // resume at the cmp's PC. See the matching Lt/Le/Eq
            // arm below for the chain-build details.
            emit_eqk_side_exit(lw, pl, oc, side_exit_pc);

            lw.bcx.switch_to_block(continue_blk);
            lw.bcx.seal_block(continue_blk);
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// Truth tests.
pub(super) fn emit_test_op<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    let OpHelpers { stack_tag_id, .. } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        Op::Test => {
            // `if (not R[A] == K) then pc++`.
            //
            // Known kind → compile-time fold (`truthy_known`
            // table). Match recorded → no IR; mismatch → bail.
            // Unset → emit runtime guard via
            // `luna_jit_stack_tag(A)` + `(tag > 1) == truthy`
            // check; runtime mismatch → deopt store_back +
            // return test.pc. The Subsequent Jmp (if TookJmp)
            // is consumed_by_cmp by the pre-emit pass.
            let a_kind = k_op(&lw.current_kinds, off as u32 + ins.a());
            let truthy_known: Option<bool> = match a_kind {
                RegKind::Int
                | RegKind::Float
                | RegKind::Table
                | RegKind::Closure
                | RegKind::Str => Some(true),
                RegKind::Nil => Some(false),
                RegKind::Unset | RegKind::Unknown | RegKind::Bool => None,
                // the trace reads it, so it is never held on the stack
                RegKind::StackHeld => return None,
            };
            let k_bit = ins.k();
            let recorded_passed = matches!(pl.cmp_dirs[i], Some(CmpDir::SkippedJmp));
            if let Some(truthy) = truthy_known {
                let test_passed = truthy != k_bit;
                if test_passed != recorded_passed {
                    // Provably can't reproduce recorded
                    // direction; bail compile.
                    return None;
                }
                // Test consumed; no IR. Match guaranteed at
                // compile time.
            } else {
                // Runtime truthy guard.
                let is_truthy = runtime_truthy(lw, oc, ins.a(), a_kind, stack_tag_id)?;
                // Op::Test: test_passed_runtime = !is_truthy == k_bit
                let not_truthy = lw.bcx.ins().bxor_imm_u(is_truthy, 1);
                let k_bit_const = lw.bcx.ins().iconst(types::I8, k_bit as i64);
                let test_passed_runtime = lw.bcx.ins().icmp(IntCC::Equal, not_truthy, k_bit_const);
                let recorded_const = lw.bcx.ins().iconst(types::I8, recorded_passed as i64);
                let ok = lw
                    .bcx
                    .ins()
                    .icmp(IntCC::Equal, test_passed_runtime, recorded_const);
                let cont = lw.bcx.create_block();
                let deopt = lw.bcx.create_block();
                lw.bcx.ins().brif(ok, cont, &[], deopt, &[]);
                lw.bcx.switch_to_block(deopt);
                lw.bcx.seal_block(deopt);
                // restored with the kinds the registers have here
                guard_exit(lw, pl, rop.pc, i);
                lw.bcx.switch_to_block(cont);
                lw.bcx.seal_block(cont);
            }
        }
        Op::TestSet => {
            // `if truthy(R[B]) == K then
            // R[A] = R[B] else pc++`.
            //
            // Known kind → compile-time fold + emit Move
            // on `TookJmp` recorded path.
            // Unset → emit runtime guard via stack_tag;
            // when match + TookJmp recorded, emit Move under
            // the `cont` block (so deopt path skips the Move).
            let b_kind = k_op(&lw.current_kinds, off as u32 + ins.b());
            let truthy_known: Option<bool> = match b_kind {
                RegKind::Int
                | RegKind::Float
                | RegKind::Table
                | RegKind::Closure
                | RegKind::Str => Some(true),
                RegKind::Nil => Some(false),
                RegKind::Unset | RegKind::Unknown | RegKind::Bool => None,
                // the trace reads it, so it is never held on the stack
                RegKind::StackHeld => return None,
            };
            let k_bit = ins.k();
            let recorded_passed = matches!(pl.cmp_dirs[i], Some(CmpDir::TookJmp));
            if let Some(truthy) = truthy_known {
                let test_passed = truthy == k_bit;
                if test_passed != recorded_passed {
                    return None;
                }
                if test_passed {
                    let v = lw.bcx.use_var(regs[ins.b() as usize]);
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    lw.current_kinds[off + ins.a() as usize] = b_kind;
                }
            } else {
                // Runtime guard. Same shape as Op::Test
                // but the basis is `is_truthy` (not `!is_truthy`).
                let is_truthy = runtime_truthy(lw, oc, ins.b(), b_kind, stack_tag_id)?;
                let k_bit_const = lw.bcx.ins().iconst(types::I8, k_bit as i64);
                let test_passed_runtime = lw.bcx.ins().icmp(IntCC::Equal, is_truthy, k_bit_const);
                let recorded_const = lw.bcx.ins().iconst(types::I8, recorded_passed as i64);
                let ok = lw
                    .bcx
                    .ins()
                    .icmp(IntCC::Equal, test_passed_runtime, recorded_const);
                let cont = lw.bcx.create_block();
                let deopt = lw.bcx.create_block();
                lw.bcx.ins().brif(ok, cont, &[], deopt, &[]);
                lw.bcx.switch_to_block(deopt);
                lw.bcx.seal_block(deopt);
                // restored with the kinds the registers have here
                guard_exit(lw, pl, rop.pc, i);
                lw.bcx.switch_to_block(cont);
                lw.bcx.seal_block(cont);
                if recorded_passed {
                    let v = lw.bcx.use_var(regs[ins.b() as usize]);
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    lw.current_kinds[off + ins.a() as usize] =
                        k_op(&lw.current_kinds, off as u32 + ins.b());
                }
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// The side exit of `EqK`, at `side_exit_pc`.
pub(super) fn emit_eqk_side_exit<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    side_exit_pc: u32,
) {
    guard_exit(lw, pl, side_exit_pc, oc.i);
}

/// Whether register `r` of op `oc`, of kind `kind`, holds a true value, as
/// an `i8`: a boolean's payload, or the tag of a head-frame register the
/// trace has not written (still on the stack). `None` for a value the
/// trace computed without knowing its type.
fn runtime_truthy<E: Emit>(
    lw: &mut Lower<E>,
    oc: &OpCx<'_>,
    r: u32,
    kind: RegKind,
    stack_tag_id: FuncId,
) -> Option<Value> {
    match kind {
        RegKind::Bool => {
            let v = lw.bcx.use_var(oc.regs[r as usize]);
            Some(lw.bcx.ins().icmp_imm_u(IntCC::NotEqual, v, 0))
        }
        RegKind::Unset | RegKind::Unknown if oc.off == 0 => {
            let slot_arg = lw.bcx.ins().iconst(types::I64, i64::from(r));
            let stack_tag_ref = lw.bcx.import_func(stack_tag_id);
            let tag_call = lw.bcx.ins().call(stack_tag_ref, &[slot_arg]);
            let tag = lw.bcx.inst_results(tag_call)[0];
            Some(lw.bcx.ins().icmp_imm_u(IntCC::UnsignedGreaterThan, tag, 1))
        }
        _ => None,
    }
}
