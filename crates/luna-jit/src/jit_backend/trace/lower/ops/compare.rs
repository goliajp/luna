use super::*;

/// Comparison with a constant.
pub(super) fn emit_eqk_op<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record,
        head_proto,
        max_stack,
        opts,
        window_size_us,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id,
        materialize_id,
        mat_sunk_id,
        ..
    } = lw.h.rt;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::EqK => {
            // `R[A] == const[B]` — Int and Float consts both
            // valid (pre-emit gated above). Emit icmp eq for
            // Int + Int, fcmp eq for Float + Float; a number
            // against the other number kind bails, against a
            // non-number is never equal.
            let bx = ins.b() as usize;
            let ka = k_op(&lw.current_kinds, off as u32 + ins.a());
            let cond = match head_proto.consts[bx] {
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
                _ => unreachable!("pre-emit gates Int / Float const only"),
            };

            let continue_blk = lw.bcx.create_block();
            let side_exit_blk = lw.bcx.create_block();
            lw.bcx
                .ins()
                .brif(cond, continue_blk, &[], side_exit_blk, &[]);

            lw.bcx.switch_to_block(side_exit_blk);
            lw.bcx.seal_block(side_exit_blk);
            let side_exit_pc = rop.pc + 2;
            // at depth>0, the side-exit must
            // materialise the inlined frames before the interp can
            // resume at the cmp's PC. See the matching Lt/Le/Eq
            // arm below for the chain-build details.
            if !lw.call_chain.is_empty() {
                // Capture head's resume pc BEFORE the innermost
                // override — `call_chain[0].pc` is the outermost
                // self-rec Call's `pc + 1` (= trace head's
                // post-Call resume).
                let head_resume_pc = lw.call_chain[0].pc;
                let mut snapshot: Vec<FrameMaterializeInfo> = lw.call_chain.clone();
                if let Some(last) = snapshot.last_mut() {
                    last.pc = side_exit_pc;
                }
                let chain_rc: TArc<[FrameMaterializeInfo]> = snapshot.into();
                let chain_ptr = TArc::as_ptr(&chain_rc) as *const FrameMaterializeInfo as i64;
                let chain_len = chain_rc.len() as i64;
                let site_idx = lw.per_exit_inline_vec.len() as u32;
                // materialise live Sinkable sites
                // BEFORE the frame_materialize_frames helper
                // pushes the inline frames. The window-sized
                // snapshot updates in-place so per_exit_inline's
                // kinds entry reflects materialised slots.
                let mut kinds_snapshot: Vec<RegKind> = lw.current_kinds.clone();
                let mat_count = emit_materialize_live_sunk(
                    &mut lw.bcx,
                    &mut lw.module,
                    mat_sunk_id,
                    &lw.escape,
                    &lw.virt_vars,
                    &lw.virt_kinds,
                    &lw.regs_full,
                    &pl.op_offsets,
                    i,
                    &mut kinds_snapshot,
                    head_proto,
                    opts.aot,
                    &mut lw.defined_aot_data,
                );
                lw.materialize_emit_count += mat_count;
                let inline_side_box_0: Box<TCellPtr> = Box::new(TCellPtr::null());
                let _inline_side_cell_addr_0 = (&*inline_side_box_0) as *const TCellPtr as i64;
                let chain_for_helper = chain_rc.clone();
                lw.per_exit_inline_vec.push((
                    side_exit_pc,
                    head_resume_pc,
                    kinds_snapshot,
                    chain_rc,
                    inline_side_box_0,
                ));
                let n_arg = lw.bcx.ins().iconst(types::I64, chain_len);
                let ptr_arg = emit_chain_ptr_arg(
                    &mut lw.module,
                    &mut lw.bcx,
                    &chain_for_helper,
                    chain_ptr,
                    opts.aot,
                    &mut lw.defined_aot_data,
                );
                let mat_ref = lw.module.declare_func_in_func(materialize_id, lw.bcx.func);
                let _ = lw.bcx.ins().call(mat_ref, &[n_arg, ptr_arg]);
                emit_store_back_and_return_site(
                    &mut lw.bcx,
                    &lw.regs_full[..window_size_us],
                    &lw.stored,
                    reg_state,
                    site_idx,
                    side_exit_pc,
                    lw.flush_ctx.as_ref(),
                    0i64,
                    trace_fn_sig_ref,
                );
            } else {
                // materialise every live
                // Sinkable site at this depth=0 cmp side-exit.
                // The snapshot carries `RegKind::Table` for each
                // materialised caller-window slot so the
                // dispatcher unpacks the heap pointer correctly
                // on deopt.
                let mut snapshot: Vec<RegKind> = lw.current_kinds[..max_stack].to_vec();
                let mat_count = emit_materialize_live_sunk(
                    &mut lw.bcx,
                    &mut lw.module,
                    mat_sunk_id,
                    &lw.escape,
                    &lw.virt_vars,
                    &lw.virt_kinds,
                    &lw.regs_full,
                    &pl.op_offsets,
                    i,
                    &mut snapshot,
                    head_proto,
                    opts.aot,
                    &mut lw.defined_aot_data,
                );
                lw.materialize_emit_count += mat_count;
                let tag_side_box_0: Box<TCellPtr> = Box::new(TCellPtr::null());
                let _tag_side_cell_addr_0 = (&*tag_side_box_0) as *const TCellPtr as i64;
                let tag_side_local_0 = lw.per_exit_kinds.len() as u32;
                lw.per_exit_kinds
                    .push((side_exit_pc, snapshot, tag_side_box_0));
                // store_back only writes caller window — depth>0 scratch
                // slots stay out of the dispatcher's reg_state restore.
                emit_tagged_exit(
                    &mut lw.bcx,
                    &mut lw.module,
                    suppress_admit_id,
                    &lw.regs_full[..max_stack],
                    &lw.stored,
                    reg_state,
                    side_exit_pc,
                    record.head_pc,
                    tag_side_local_0,
                    lw.flush_ctx.as_ref(),
                    trace_fn_sig_ref,
                );
            }

            lw.bcx.switch_to_block(continue_blk);
            lw.bcx.seal_block(continue_blk);
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// Truth tests.
pub(super) fn emit_test_op<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let OpHelpers { stack_tag_id, .. } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
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
                RegKind::Unset | RegKind::Unknown => None,
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
                // Runtime tag-based truthy guard.
                let slot_arg = lw.bcx.ins().iconst(types::I64, ins.a() as i64);
                let stack_tag_ref = lw.module.declare_func_in_func(stack_tag_id, lw.bcx.func);
                let tag_call = lw.bcx.ins().call(stack_tag_ref, &[slot_arg]);
                let tag = lw.bcx.inst_results(tag_call)[0];
                let one = lw.bcx.ins().iconst(types::I64, 1);
                let is_truthy = lw.bcx.ins().icmp(IntCC::UnsignedGreaterThan, tag, one);
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
                RegKind::Unset | RegKind::Unknown => None,
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
                let slot_arg = lw.bcx.ins().iconst(types::I64, ins.b() as i64);
                let stack_tag_ref = lw.module.declare_func_in_func(stack_tag_id, lw.bcx.func);
                let tag_call = lw.bcx.ins().call(stack_tag_ref, &[slot_arg]);
                let tag = lw.bcx.inst_results(tag_call)[0];
                let one = lw.bcx.ins().iconst(types::I64, 1);
                let is_truthy = lw.bcx.ins().icmp(IntCC::UnsignedGreaterThan, tag, one);
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
