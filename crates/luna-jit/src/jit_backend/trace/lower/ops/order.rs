use super::*;

/// Register comparisons.
pub(super) fn emit_order_op<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan { head_proto, .. } = *pl;
    let OpCx {
        i, rop, ins, op, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::Lt | Op::Le | Op::Eq => {
            // Lua semantics: `if (R[A] op R[B]) ~= K then pc++`.
            // Operand kinds must match — Int → icmp; Float →
            // fcmp; mixed → bail.
            //
            // `cmp_dirs[i]` decides which direction the cmp
            // recorded: TookJmp (cond folds K so true ⇒ took
            // Jmp → continue) or SkippedJmp (cond folds !K so
            // true ⇒ skipped Jmp → continue; side-exit goes
            // to the Jmp's target). The pre-emit pass already
            // validated `i+1 < effective_end`.
            let dir = pl.cmp_dirs[i].expect("cmp dir set in pre-emit");
            let invert = matches!(dir, CmpDir::SkippedJmp);
            let k_effective = if invert { !ins.k() } else { ins.k() };
            let ka = oc.kind(&lw.current_kinds, ins.a());
            let kb = oc.kind(&lw.current_kinds, ins.b());
            let float_path = matches!(ka, RegKind::Float) || matches!(kb, RegKind::Float);
            let cond = if float_path {
                if !matches!(ka, RegKind::Float) || !matches!(kb, RegKind::Float) {
                    return None;
                }
                let lhs = use_var_f64(&mut lw.bcx, regs, ins.a());
                let rhs = use_var_f64(&mut lw.bcx, regs, ins.b());
                let float_cc = match op {
                    Op::Lt => FloatCC::LessThan,
                    Op::Le => FloatCC::LessThanOrEqual,
                    Op::Eq => FloatCC::Equal,
                    _ => unreachable!("whitelist gated above"),
                };
                let c = lw.bcx.ins().fcmp(float_cc, lhs, rhs);
                // negate the ordered compare rather than flip the
                // condition: `not (a < b)` holds for NaN, `a >= b`
                // does not, and the aarch64 backend lowers no
                // unordered-or conditions
                if k_effective {
                    c
                } else {
                    lw.bcx.ins().icmp_imm_u(IntCC::Equal, c, 0)
                }
            } else if op == Op::Eq {
                emit_eq_cond(lw, pl, oc, ka, kb, k_effective)
            } else {
                // only integers order by their payload
                match (ka, kb) {
                    (RegKind::Int, RegKind::Int) => {}
                    (
                        RegKind::Unset | RegKind::Unknown,
                        RegKind::Int | RegKind::Unset | RegKind::Unknown,
                    )
                    | (RegKind::Int, RegKind::Unset | RegKind::Unknown) => {
                        lw.dispatchable = false;
                        lw.dispatch_off_reason =
                            lw.dispatch_off_reason.or(Some("cmp:unknown-kind"));
                    }
                    _ => return None,
                }
                let lhs = lw.bcx.use_var(regs[ins.a() as usize]);
                let rhs = lw.bcx.use_var(regs[ins.b() as usize]);
                let int_cc = match (op, k_effective) {
                    (Op::Lt, true) => IntCC::SignedLessThan,
                    (Op::Lt, false) => IntCC::SignedGreaterThanOrEqual,
                    (Op::Le, true) => IntCC::SignedLessThanOrEqual,
                    (Op::Le, false) => IntCC::SignedGreaterThan,
                    _ => unreachable!("whitelist gated above"),
                };
                lw.bcx.ins().icmp(int_cc, lhs, rhs)
            };

            let continue_blk = lw.bcx.create_block();
            let side_exit_blk = lw.bcx.create_block();
            lw.bcx
                .ins()
                .brif(cond, continue_blk, &[], side_exit_blk, &[]);

            // Side-exit PC depends on the recorded direction:
            //   TookJmp    → interp's `pc++` lands at cmp_pc + 2.
            //   SkippedJmp → interp would have taken the Jmp;
            //                resume at the Jmp's target.
            let side_exit_pc: u32 = match dir {
                CmpDir::TookJmp => rop.pc + 2,
                CmpDir::SkippedJmp => {
                    let jmp_pc = (rop.pc + 1) as usize;
                    let jmp_inst = head_proto.code[jmp_pc];
                    let pc_after_jmp = (rop.pc as i64) + 2;
                    (pc_after_jmp + jmp_inst.sj() as i64) as u32
                }
            };
            lw.bcx.switch_to_block(side_exit_blk);
            lw.bcx.seal_block(side_exit_blk);
            // at depth>0, snapshot the live
            // `call_chain` (each cmp@d>0 site has its OWN chain;
            // a single global depth-indexed array loops fib
            // forever because sibling Calls produce wrong
            // chains under the depth-indexed lookup). The
            // innermost frame's pc is overwritten with this
            // site's side-exit PC so the materialize helper
            // stays PC-agnostic — it just pushes whatever
            // metadata says.
            emit_cmp_side_exit(lw, pl, oc, side_exit_pc);

            // Continue: subsequent ops emit here.
            lw.bcx.switch_to_block(continue_blk);
            lw.bcx.seal_block(continue_blk);
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// The side exit of a register comparison, at `side_exit_pc`.
pub(super) fn emit_cmp_side_exit<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    side_exit_pc: u32,
) {
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
    let OpCx { i, .. } = *oc;
    if !lw.call_chain.is_empty() {
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
        // (depth=0 + depth>0) before frame-mat helper
        // pushes the inline frames.
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
        let inline_side_box_1: Box<TCellPtr> = Box::new(TCellPtr::null());
        let _inline_side_cell_addr_1 = (&*inline_side_box_1) as *const TCellPtr as i64;
        let chain_for_helper = chain_rc.clone();
        lw.per_exit_inline_vec.push((
            side_exit_pc,
            head_resume_pc,
            kinds_snapshot,
            chain_rc,
            inline_side_box_1,
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
        // materialise-on-deopt for
        // depth=0 cmp's live Sinkable sites.
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
        let tag_side_box_1: Box<TCellPtr> = Box::new(TCellPtr::null());
        let _tag_side_cell_addr_1 = (&*tag_side_box_1) as *const TCellPtr as i64;
        let tag_side_local_1 = lw.per_exit_kinds.len() as u32;
        lw.per_exit_kinds
            .push((side_exit_pc, snapshot, tag_side_box_1));
        emit_tagged_exit(
            &mut lw.bcx,
            &mut lw.module,
            suppress_admit_id,
            &lw.regs_full[..max_stack],
            &lw.stored,
            reg_state,
            side_exit_pc,
            record.head_pc,
            tag_side_local_1,
            lw.flush_ctx.as_ref(),
            trace_fn_sig_ref,
        );
    }
}

/// The condition of `==` on two non-float registers of kinds `ka` and `kb`.
pub(super) fn emit_eq_cond<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    ka: RegKind,
    kb: RegKind,
    k_effective: bool,
) -> Value {
    let OpCx { i, rop, ins, .. } = *oc;
    let regs: &[Variable] = &oc.regs;
    let lhs = lw.bcx.use_var(regs[ins.a() as usize]);
    let rhs = lw.bcx.use_var(regs[ins.b() as usize]);
    let int_cc = if k_effective {
        IntCC::Equal
    } else {
        IntCC::NotEqual
    };
    match eq_lowering(ka, kb) {
        EqLowering::Payload => lw.bcx.ins().icmp(int_cc, lhs, rhs),
        EqLowering::Unequal => lw.bcx.ins().iconst(types::I8, i64::from(!k_effective)),
        EqLowering::Identity(kind) => {
            // two distinct objects can still be equal (`__eq`,
            // equal long strings); the interpreter decides those
            let same = lw.bcx.ins().icmp(IntCC::Equal, lhs, rhs);
            let decided = if kind == RegKind::Table {
                let no_mt = |bcx: &mut FunctionBuilder<'_>, t| {
                    let mt = bcx.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        t,
                        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
                    );
                    bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0)
                };
                let l = no_mt(&mut lw.bcx, lhs);
                let r = no_mt(&mut lw.bcx, rhs);
                lw.bcx.ins().band(l, r)
            } else {
                let short = |bcx: &mut FunctionBuilder<'_>, s| {
                    let b = bcx.ins().load(
                        types::I8,
                        MemFlagsData::trusted(),
                        s,
                        crate::jit_backend::STR_SHORT_OFFSET as i32,
                    );
                    bcx.ins().icmp_imm_u(IntCC::NotEqual, b, 0)
                };
                let l = short(&mut lw.bcx, lhs);
                let r = short(&mut lw.bcx, rhs);
                lw.bcx.ins().band(l, r)
            };
            let ok = lw.bcx.ins().bor(same, decided);
            guard!(lw, pl, ok, i, rop.pc);
            lw.bcx.ins().icmp(int_cc, lhs, rhs)
        }
        EqLowering::Unknown => {
            lw.dispatchable = false;
            lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("cmp:unknown-kind"));
            lw.bcx.ins().icmp(int_cc, lhs, rhs)
        }
    }
}
