use super::*;

/// Checks once, before the loop head, that each folded `math.<fn>` is
/// still the library function (only when nothing in the trace can
/// reassign it).
pub(super) fn emit_fold_precheck<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Plan {
        record,
        head_proto,
        max_stack,
        opts,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        precheck,
        body_loop,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id,
        math_fn_check_id,
        ..
    } = lw.h.rt;
    if let Some(precheck) = precheck {
        // Nothing has run yet: a failed check leaves at the head with the
        // entry kinds, and the interpreter makes the calls.
        lw.bcx.switch_to_block(precheck);
        lw.bcx.seal_block(precheck);
        // Before the loop head reg_state holds what the prelude loaded.
        let entry_stored: Vec<Option<Value>> = lw
            .regs_full
            .iter()
            .map(|&v| Some(lw.bcx.use_var(v)))
            .collect();
        // interned, so one pointer per name
        let mut checked: Vec<*const u8> = Vec::new();
        for fold in &pl.math_folds {
            let math_key = head_proto.consts[record.ops[fold.start_idx].inst.c() as usize];
            let name_key = head_proto.consts[record.ops[fold.start_idx + 1].inst.c() as usize];
            let (
                luna_core::runtime::Value::Str(math_key),
                luna_core::runtime::Value::Str(name_key),
            ) = (math_key, name_key)
            else {
                unreachable!("the fold matcher took both keys as strings");
            };
            let name_ptr = name_key.as_ptr() as *const u8;
            if checked.contains(&name_ptr) {
                continue;
            }
            checked.push(name_ptr);
            let m = emit_str_key_arg(&mut lw.bcx, math_key, opts.aot, &mut lw.defined_aot_data);
            let k = emit_str_key_arg(&mut lw.bcx, name_key, opts.aot, &mut lw.defined_aot_data);
            let check_ref = lw.bcx.import_func(math_fn_check_id);
            let call = lw.bcx.ins().call(check_ref, &[m, k]);
            let is_library = lw.bcx.inst_results(call)[0];
            let ok_blk = lw.bcx.create_block();
            let exit_blk = lw.bcx.create_block();
            lw.bcx.ins().brif(is_library, ok_blk, &[], exit_blk, &[]);
            lw.bcx.switch_to_block(exit_blk);
            lw.bcx.seal_block(exit_blk);
            let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
            let tags_idx = lw.per_exit_kinds.len() as u32;
            lw.per_exit_kinds.push((
                record.head_pc,
                lw.current_kinds[..max_stack].to_vec(),
                side_box,
            ));
            emit_tagged_exit(
                &mut lw.bcx,
                suppress_admit_id,
                &lw.regs_full[..max_stack],
                &entry_stored,
                reg_state,
                record.head_pc,
                record.head_pc,
                tags_idx,
                lw.flush_ctx.as_ref(),
                trace_fn_sig_ref,
            );
            lw.bcx.switch_to_block(ok_blk);
            lw.bcx.seal_block(ok_blk);
        }
        lw.bcx.ins().jump(lw.ro_precheck.unwrap_or(body_loop), &[]);
    }
}

/// Lowers op `oc.i` of a math fold: the folded call at its emit position,
/// nothing at the silent ones.
pub(super) fn emit_fold<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    let Plan {
        record,
        head_proto,
        opts,
        ..
    } = *pl;
    let Lower { precheck, .. } = *lw;
    let RuntimeHelpers {
        math_fn_check_id, ..
    } = lw.h.rt;
    let OpCx { i, rop, off, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    // Resolve which fold this index belongs to: the start
    // (Libm1 emit site or Min2/Max2 silent GetTabUp), the
    // GetField mid-op (Min2/Max2 silent), or the Call
    // (Min2/Max2 emit site).
    let fold = pl.math_folds.iter().find(|f| {
        f.start_idx == i || (f.kind.split() && (f.start_idx + 1 == i || f.call_idx == i))
    });
    if let Some(fold) = fold {
        // The fold stands for the library function; leave the
        // trace at the GetTabUp, before anything of the call
        // has run, when `math.<fn>` holds something else (checked
        // in `precheck` instead when nothing in the trace can
        // change the field).
        if fold.start_idx == i && precheck.is_none() {
            let math_key = head_proto.consts[record.ops[i].inst.c() as usize];
            let name_key = head_proto.consts[record.ops[i + 1].inst.c() as usize];
            let (
                luna_core::runtime::Value::Str(math_key),
                luna_core::runtime::Value::Str(name_key),
            ) = (math_key, name_key)
            else {
                unreachable!("the fold matcher took both keys as strings");
            };
            let m = emit_str_key_arg(&mut lw.bcx, math_key, opts.aot, &mut lw.defined_aot_data);
            let k = emit_str_key_arg(&mut lw.bcx, name_key, opts.aot, &mut lw.defined_aot_data);
            let check_ref = lw.bcx.import_func(math_fn_check_id);
            let call = lw.bcx.ins().call(check_ref, &[m, k]);
            let is_library = lw.bcx.inst_results(call)[0];
            guard!(lw, pl, is_library, i, rop.pc);
        }
        match fold.kind {
            FoldKind::Libm1 if fold.start_idx == i => {
                // Declare libm fn fresh per fold (cranelift
                // dedups by name in the same module).
                let mut libm_sig = lw.bcx.make_signature();
                libm_sig.params.push(AbiParam::new(types::F64));
                libm_sig.returns.push(AbiParam::new(types::F64));
                let libm_id = lw
                    .bcx
                    .declare_function(fold.fn_name, Linkage::Import, &libm_sig)
                    .ok()?;
                let libm_ref = lw.bcx.import_func(libm_id);
                // Libm1 always has a Reg arg_src — coerce
                // to f64 via the existing Int→f64 / bitcast
                // ladder based on current_kinds.
                let arg_src = fold.arg_src.expect("Libm1 has arg_src");
                let FoldArgSrc::Reg { reg: arg_reg } = arg_src;
                let arg_kind = k_op(&lw.current_kinds, off as u32 + arg_reg);
                // The argument must be a number the trace knows as
                // one: a numeric string is valid Lua here, and its
                // payload is a pointer.
                if !matches!(arg_kind, RegKind::Int | RegKind::Float) {
                    return None;
                }
                if is_rounding(fold.fn_name) {
                    // 5.4+: an integer is its own floor/ceil; a
                    // float's becomes an integer when it fits.
                    // When it does not (NaN, the infinities,
                    // beyond ±2^63) the result is a float, and
                    // the trace leaves at the GetTabUp — nothing
                    // of the call has run — for the interpreter.
                    let raw = lw.bcx.use_var(regs[arg_reg as usize]);
                    let r = if matches!(arg_kind, RegKind::Float) {
                        let x = use_var_f64(&mut lw.bcx, regs, arg_reg);
                        let r = if fold.fn_name == "floor" {
                            lw.bcx.ins().floor(x)
                        } else {
                            lw.bcx.ins().ceil(x)
                        };
                        let fits = emit_f64_fits_i64(&mut lw.bcx, r);
                        guard!(lw, pl, fits, i, rop.pc);
                        lw.bcx.ins().fcvt_to_sint(types::I64, r)
                    } else {
                        raw
                    };
                    lw.bcx.def_var(regs[fold.dst_reg as usize], r);
                    lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Int;
                    return Some(());
                }
                let arg_f64 = if matches!(arg_kind, RegKind::Float) {
                    use_var_f64(&mut lw.bcx, regs, arg_reg)
                } else {
                    let raw = lw.bcx.use_var(regs[arg_reg as usize]);
                    lw.bcx.ins().fcvt_from_sint(types::F64, raw)
                };
                let call = if fold.fn_name == "atan" {
                    // Only on 5.4+ (see the matcher): atan2(y, 1).
                    let mut atan2_sig = lw.bcx.make_signature();
                    atan2_sig.params.push(AbiParam::new(types::F64));
                    atan2_sig.params.push(AbiParam::new(types::F64));
                    atan2_sig.returns.push(AbiParam::new(types::F64));
                    let atan2_id = lw
                        .bcx
                        .declare_function("atan2", Linkage::Import, &atan2_sig)
                        .ok()?;
                    let atan2_ref = lw.bcx.import_func(atan2_id);
                    let one = lw.bcx.ins().f64const(1.0);
                    lw.bcx.ins().call(atan2_ref, &[arg_f64, one])
                } else {
                    lw.bcx.ins().call(libm_ref, &[arg_f64])
                };
                let r = lw.bcx.inst_results(call)[0];
                def_var_f64(&mut lw.bcx, regs[fold.dst_reg as usize], r);
                lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Float;
            }
            FoldKind::Libm1 => {
                // Libm1 silent trailer (Move / Call) — folded
                // away, no IR.
            }
            FoldKind::Min2 | FoldKind::Max2 if fold.call_idx == i => {
                emit_minmax_fold(lw, pl, oc, fold)?;
            }
            FoldKind::StrSub if fold.call_idx == i => {
                emit_str_sub_fold(lw, pl, oc, fold)?;
            }
            FoldKind::Min2 | FoldKind::Max2 | FoldKind::StrSub => {
                // Silent: this index is either `start_idx`
                // (GetTabUp) or `start_idx + 1` (GetField).
                // The Call's emit will fire at `call_idx`
                // and produce the fold's IR.
            }
        }
    }
    Some(())
}

/// A `string.sub` fold at its call: on a string and numbers the trace
/// knows as such; anything else is not compiled. A float position equal
/// to an integer is that integer in every dialect; any other float (which
/// 5.1 / 5.2 round and 5.3+ reject) leaves the trace at the fold's start,
/// whose argument set-up only writes the call's registers.
fn emit_str_sub_fold<E: Emit>(
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
                (FoldKind::Libm1 | FoldKind::StrSub, _) => unreachable!(),
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
            FoldKind::Libm1 | FoldKind::StrSub => unreachable!(),
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
            FoldKind::Libm1 | FoldKind::StrSub => unreachable!(),
        };
        lw.bcx.def_var(regs[fold.dst_reg as usize], r);
        lw.current_kinds[off + fold.dst_reg as usize] = RegKind::Int;
    }
    Some(())
}
