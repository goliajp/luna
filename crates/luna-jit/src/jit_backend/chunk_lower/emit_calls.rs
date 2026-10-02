use super::*;

/// Returns the pc emit continues from: the fold spans four ops.
pub(super) fn emit_math_fold<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
) -> Option<usize> {
    let EmitFacts {
        c: ChunkIn {
            code, float_only, ..
        },
        scan,
        reg_kinds,
        regs,
        ..
    } = f;
    let ChunkScan {
        folded_math,
        math_folds,
        ..
    } = scan;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    let mut pc = pc;
    let ins = code[pc];
    match ins.op() {
        Op::GetTabUp => {
            // emit-side fold consumer. PCs +1..+3 are
            // also folded; the outer loop advances `pc` by 3 (plus
            // the trailing `pc += 1`) so we skip past `GetField`,
            // `Move`, and the `Call`.
            debug_assert!(
                folded_math[pc],
                "scanner accepts GetTabUp only inside a math fold"
            );
            let fold = math_folds
                .iter()
                .find(|f| f.start_pc == pc)
                .copied()
                .expect("math fold for this PC");

            let arg_kind = a_kind(reg_kinds, fold.arg_reg);
            let arg_var = bcx.use_var(regs[fold.arg_reg as usize]);
            let result = if fold.int_result {
                match arg_kind {
                    RegKind::Float => {
                        let r = if fold.fn_name == "floor" {
                            bcx.ins().floor(arg_var)
                        } else {
                            bcx.ins().ceil(arg_var)
                        };
                        // An integer when it fits (NaN and the
                        // infinities do not); otherwise the result
                        // is a float, a kind this register cannot
                        // hold, and the interpreter reruns the call.
                        // Folds only compile in chunks without
                        // table stores, so nothing has happened yet
                        // that a rerun would repeat.
                        let lo = bcx.ins().f64const(-9_223_372_036_854_775_808.0);
                        let hi = bcx.ins().f64const(9_223_372_036_854_775_808.0);
                        let ge_lo = bcx.ins().fcmp(FloatCC::GreaterThanOrEqual, r, lo);
                        let lt_hi = bcx.ins().fcmp(FloatCC::LessThan, r, hi);
                        let fits = bcx.ins().band(ge_lo, lt_hi);
                        let ok_blk = bcx.create_block();
                        let bail_blk = bcx.create_block();
                        bcx.ins().brif(fits, ok_blk, &[], bail_blk, &[]);
                        bcx.switch_to_block(bail_blk);
                        bcx.seal_block(bail_blk);
                        let park_id = module
                            .declare_function(
                                "luna_jit_park_deopt",
                                Linkage::Import,
                                &module.make_signature(),
                            )
                            .ok()?;
                        let park_ref = module.declare_func_in_func(park_id, bcx.func);
                        bcx.ins().call(park_ref, &[]);
                        let zero = bcx.ins().iconst(types::I64, 0);
                        bcx.ins().return_(&[zero]);
                        bcx.switch_to_block(ok_blk);
                        bcx.seal_block(ok_blk);
                        bcx.ins().fcvt_to_sint(types::I64, r)
                    }
                    // An integer is its own floor and ceiling.
                    RegKind::Int | RegKind::Unset => arg_var,
                    // `math.floor(t)` raises in the interpreter
                    RegKind::Table => return None,
                }
            } else {
                let arg_f64 = match arg_kind {
                    RegKind::Float => arg_var,
                    RegKind::Int | RegKind::Unset => bcx.ins().fcvt_from_sint(types::F64, arg_var),
                    // `math.sin(t)` raises in the interpreter
                    RegKind::Table => return None,
                };
                // 5.3+ `atan(y)` is `atan2(y, 1)` (lmathlib.c), which
                // libm rounds differently from `atan(y)`.
                let atan2 = fold.fn_name == "atan" && !float_only;
                let mut libm_sig = module.make_signature();
                libm_sig.params.push(AbiParam::new(types::F64));
                if atan2 {
                    libm_sig.params.push(AbiParam::new(types::F64));
                }
                libm_sig.returns.push(AbiParam::new(types::F64));
                let name = if atan2 { "atan2" } else { fold.fn_name };
                let libm_id = module
                    .declare_function(name, Linkage::Import, &libm_sig)
                    .ok()?;
                let libm_ref = module.declare_func_in_func(libm_id, bcx.func);
                let call_inst = if atan2 {
                    let one = bcx.ins().f64const(1.0);
                    bcx.ins().call(libm_ref, &[arg_f64, one])
                } else {
                    bcx.ins().call(libm_ref, &[arg_f64])
                };
                bcx.inst_results(call_inst)[0]
            };
            aligned_def(bcx, regs, reg_kinds, fold.dst_reg as usize, result);
            current_kinds[fold.dst_reg as usize] = fold.result_kind();
            current_is_nil[fold.dst_reg as usize] = false;

            pc += 3; // skip GetField + Move + Call; outer `pc += 1` lands past the Call.
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(pc)
}

pub(super) fn emit_get_upval<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        scan,
        reg_kinds,
        regs,
        ..
    } = f;
    let ChunkScan {
        is_upval_value_read,
        ..
    } = scan;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::GetUpval => {
            let a = ins.a() as usize;
            if is_upval_value_read[pc] {
                // ValueRead: fetch the upvalue at
                // runtime via `luna_jit_upval_get_float`, which deopts
                // on anything but a float. The dispatcher
                // has pinned `JIT_CL` to the active closure for
                // this entry, so the helper can resolve the
                // upvalue cell. Result is the raw 8-byte payload;
                // `aligned_def` bitcasts to F64 since the sweep
                // pinned reg_kinds[a] = Float.
                let idx_arg = bcx.ins().iconst(types::I64, ins.b() as i64);
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_upval_get_float", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call_inst = bcx.ins().call(r, &[idx_arg]);
                let v = bcx.inst_results(call_inst)[0];
                aligned_def(bcx, regs, reg_kinds, a, v);
                current_kinds[a] = reg_kinds[a];
                current_is_nil[a] = false;
            } else {
                // SelfMarker placeholder. The matching
                // Op::Call gets rewritten to a direct cranelift
                // call; this register's value is never read.
                let zero = if matches!(a_kind(reg_kinds, ins.a()), RegKind::Float) {
                    bcx.ins().f64const(0.0)
                } else {
                    bcx.ins().iconst(types::I64, 0)
                };
                aligned_def(bcx, regs, reg_kinds, a, zero);
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

pub(super) fn emit_self_call<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        scan,
        reg_kinds,
        ret_kind,
        regs,
        fn_id,
        ..
    } = f;
    let ChunkScan { self_call_pcs, .. } = scan;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::Call => {
            debug_assert!(
                self_call_pcs[pc],
                "scanner accepts only self-recursive Calls"
            );
            let a = ins.a() as usize;
            let nargs = (ins.b() - 1) as usize;
            let mut arg_vals: Vec<Value> = Vec::with_capacity(nargs);
            for i in 0..nargs {
                let slot_idx = a + 1 + i;
                let v = bcx.use_var(regs[slot_idx]);
                // The cranelift call sig matches the entry sig
                // (all i64). Bitcast Float args back to i64 at
                // the call boundary.
                let v_i64 = if matches!(a_kind(reg_kinds, slot_idx as u32), RegKind::Float) {
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), v)
                } else {
                    v
                };
                arg_vals.push(v_i64);
            }
            let self_ref = module.declare_func_in_func(fn_id, bcx.func);
            let call_inst = bcx.ins().call(self_ref, &arg_vals);
            let result_i64 = bcx.inst_results(call_inst)[0];
            // Self-call result is `ret_kind`; bitcast back to
            // F64 if Float. Pre-write `current_kinds[a]` would
            // be stale here.
            let result = if matches!(ret_kind, RegKind::Float) {
                bcx.ins()
                    .bitcast(types::F64, MemFlagsData::new(), result_i64)
            } else {
                result_i64
            };
            aligned_def(bcx, regs, reg_kinds, a, result);
            // self-recursive call returns ret_kind.
            if !matches!(ret_kind, RegKind::Unset) {
                current_kinds[a] = ret_kind;
            }
            current_is_nil[a] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
