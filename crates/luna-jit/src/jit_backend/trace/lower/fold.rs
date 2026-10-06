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
        let next = lw.ro_precheck.or(lw.step_precheck).unwrap_or(body_loop);
        lw.bcx.ins().jump(next, &[]);
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
            FoldKind::Fmod2 if fold.call_idx == i => {
                emit_fmod_fold(lw, pl, oc, fold)?;
            }
            FoldKind::Min2 | FoldKind::Max2 | FoldKind::StrSub | FoldKind::Fmod2 => {
                // Silent: this index is either `start_idx`
                // (GetTabUp) or `start_idx + 1` (GetField).
                // The Call's emit will fire at `call_idx`
                // and produce the fold's IR.
            }
        }
    }
    Some(())
}
