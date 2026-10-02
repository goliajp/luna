use super::*;

pub(super) fn emit_get_i<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds, regs, ..
    } = f;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::GetI => {
            // `R[A] = R[B][imm(C)]`. Inline
            // aget fast path: when the immediate `C` key fits the
            // array part AND the table has no metatable, load the
            // raw 8-byte payload from `array_ptr[key-1] * 8`
            // directly. Mirrors the inline aset shape:
            //   if (key - 1) as u64 < asize AND metatable.is_none()
            //     avals_ptr = load array_ptr
            //     bits = load i64 at avals_ptr + (key - 1) * 8
            //     def R[A] = bits
            //   else
            //     bits = luna_jit_table_get_int(t, key)
            // The slow path covers out-of-bounds keys (hash part)
            // and metatable'd tables (helper sets pending_err →
            // dispatcher deopts to interp).
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let t_raw = bcx.use_var(regs[b]);
            let t = if matches!(
                reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                RegKind::Float
            ) {
                bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
            } else {
                t_raw
            };
            let key_imm = ins.c() as i64;

            let asize = bcx.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                t,
                TABLE_ASIZE_OFFSET as i32,
            );
            let key_minus_1 = bcx.ins().iconst(types::I64, key_imm - 1);
            let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
            let metatable = bcx.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                t,
                TABLE_METATABLE_OFFSET as i32,
            );
            let zero_i64 = bcx.ins().iconst(types::I64, 0);
            let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero_i64);
            let fast_ok = bcx.ins().band(in_range, no_meta);

            let key = bcx.ins().iconst(types::I64, key_imm);
            let want = want_tag(reg_kinds.get(a).copied().unwrap_or(RegKind::Int));
            let v = emit_checked_get(
                bcx,
                module,
                t,
                fast_ok,
                key_minus_1,
                ("luna_jit_table_get_int_checked", key),
                want,
            )?;
            aligned_def(bcx, regs, reg_kinds, a, v);
            current_kinds[a] = reg_kinds[a];
            current_is_nil[a] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

pub(super) fn emit_get_table<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds, regs, ..
    } = f;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::GetTable => {
            // `R[A] = R[B][R[C]]`. Same fast
            // path shape as the GetI inline aget, but the
            // key sits in a register rather than as an immediate.
            // Float keys (5.1/5.2 `t[1.0]`) get an exactness check
            // (in the i64 range, and fcvt + fcvt back == original)
            // before the bounds + metatable guards; NaN, infinite,
            // out-of-range and fractional keys fall through to the
            // helper which walks the hash part. Int keys (5.3+) skip
            // the fcvt round-trip.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            let t_raw = bcx.use_var(regs[b]);
            let t = if matches!(
                reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                RegKind::Float
            ) {
                bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
            } else {
                t_raw
            };
            let key_raw = bcx.use_var(regs[c]);
            let key_kind = a_kind(reg_kinds, c as u32);
            let is_float_key = matches!(key_kind, RegKind::Float);

            // Compute (key_i64, exact_or_int_key) where exact_or_int_key
            // is the fast-path eligibility flag for the key's
            // numeric form.
            let (key_i64, key_ok) = if is_float_key {
                // the saturating form: the trapping one kills the
                // process on a NaN or out-of-range key
                let key_int = bcx.ins().fcvt_to_sint_sat(types::I64, key_raw);
                let key_back = bcx.ins().fcvt_from_sint(types::F64, key_int);
                let round_trips = bcx.ins().fcmp(FloatCC::Equal, key_raw, key_back);
                // 2^63 saturates to i64::MAX, which converts back to
                // 2^63: only the range check tells it apart
                let fits = trace::emit_f64_fits_i64(bcx, key_raw);
                let exact = bcx.ins().band(round_trips, fits);
                (key_int, exact)
            } else {
                // Int key — always "exact" by construction.
                let always = bcx.ins().iconst(types::I8, 1);
                (key_raw, always)
            };

            let asize = bcx.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                t,
                TABLE_ASIZE_OFFSET as i32,
            );
            let one = bcx.ins().iconst(types::I64, 1);
            let key_minus_1 = bcx.ins().isub(key_i64, one);
            let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
            let metatable = bcx.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                t,
                TABLE_METATABLE_OFFSET as i32,
            );
            let zero_i64 = bcx.ins().iconst(types::I64, 0);
            let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero_i64);
            let bounds_ok = bcx.ins().band(in_range, no_meta);
            let fast_ok = bcx.ins().band(bounds_ok, key_ok);

            let slow = if is_float_key {
                let key_bits = bcx.ins().bitcast(types::I64, MemFlagsData::new(), key_raw);
                ("luna_jit_table_get_float_checked", key_bits)
            } else {
                ("luna_jit_table_get_int_checked", key_raw)
            };
            let want = want_tag(reg_kinds.get(a).copied().unwrap_or(RegKind::Int));
            let v = emit_checked_get(bcx, module, t, fast_ok, key_minus_1, slow, want)?;
            aligned_def(bcx, regs, reg_kinds, a, v);
            current_kinds[a] = reg_kinds[a];
            current_is_nil[a] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

pub(super) fn emit_len<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds, regs, ..
    } = f;
    let EmitState { current_kinds, .. } = st;
    match ins.op() {
        Op::Len => {
            // `R[A] = #R[B]`.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let t_raw = bcx.use_var(regs[b]);
            // Float-declared table operand
            // bitcast to I64; see SetTable.
            let t = if matches!(
                reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                RegKind::Float
            ) {
                bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
            } else {
                t_raw
            };
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = module
                .declare_function("luna_jit_table_len", Linkage::Import, &sig)
                .ok()?;
            let r = module.declare_func_in_func(id, bcx.func);
            let call_inst = bcx.ins().call(r, &[t]);
            let v = bcx.inst_results(call_inst)[0];
            aligned_def(bcx, regs, reg_kinds, a, v);
            current_kinds[a] = RegKind::Int;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
