use super::*;

/// The tag a method-JIT register of `kind` holds, as `Value::unpack`
/// reports it.
pub(super) fn want_tag(kind: RegKind) -> i64 {
    match kind {
        RegKind::Int | RegKind::Unset => RAW_TAG_INT,
        RegKind::Float => RAW_TAG_FLOAT,
        RegKind::Table => RAW_TAG_TABLE,
    }
}

/// A typed table read, `R[A] = t[key]`, whose register kind was inferred
/// statically: the value's tag is checked against `want`, and a value of
/// another type (nil for a missing key, a string, ...) leaves the compiled
/// call so the interpreter re-runs it, as a metatable does. Reading the
/// raw payload unchecked turned a nil into integer 0 or float 0.0.
///
/// After an inline store of a non-nil value into array slot `idx` whose
/// tag was `old_tag`, keep `Table`'s `acount` / `aprefix` as `aset` does
/// (a nil slot turning non-nil counts, and extends a leading run that ends
/// exactly there), then continue at `next`.
pub(super) fn emit_array_fill_count(
    bcx: &mut FunctionBuilder,
    t: Value,
    idx: Value,
    old_tag: Value,
    next: Block,
) {
    let count_blk = bcx.create_block();
    let was_nil = bcx.ins().icmp_imm_u(IntCC::Equal, old_tag, RAW_TAG_NIL);
    bcx.ins().brif(was_nil, count_blk, &[], next, &[]);
    bcx.switch_to_block(count_blk);
    bcx.seal_block(count_blk);
    let flags = MemFlagsData::trusted();
    let acount = bcx.ins().load(types::I32, flags, t, TABLE_ACOUNT_OFFSET);
    let acount = bcx.ins().iadd_imm_u(acount, 1);
    bcx.ins().store(flags, acount, t, TABLE_ACOUNT_OFFSET);
    let aprefix = bcx.ins().load(types::I32, flags, t, TABLE_APREFIX_OFFSET);
    let wide = bcx.ins().uextend(types::I64, aprefix);
    let at_end = bcx.ins().icmp(IntCC::Equal, wide, idx);
    let grown = bcx.ins().iadd_imm_u(aprefix, 1);
    let aprefix = bcx.ins().select(at_end, grown, aprefix);
    bcx.ins().store(flags, aprefix, t, TABLE_APREFIX_OFFSET);
    bcx.ins().jump(next, &[]);
}

/// `fast_ok` selects the inline array read (`key - 1` in range, no
/// metatable); otherwise `slow` names a `*_checked` helper and its key.
pub(super) fn emit_checked_get<M: Module>(
    bcx: &mut FunctionBuilder<'_>,
    module: &mut M,
    t: Value,
    fast_ok: Value,
    key_minus_1: Value,
    slow: (&str, Value),
    want: i64,
) -> Option<Value> {
    let fast_blk = bcx.create_block();
    let slow_blk = bcx.create_block();
    let deopt_blk = bcx.create_block();
    let merge_blk = bcx.create_block();
    bcx.append_block_param(merge_blk, types::I64);
    bcx.ins().brif(fast_ok, fast_blk, &[], slow_blk, &[]);

    // atags trail the avals: the tag of slot i is at avals_ptr + asize * 8 + i
    bcx.switch_to_block(fast_blk);
    bcx.seal_block(fast_blk);
    let avals_ptr = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        TABLE_ARRAY_PTR_OFFSET as i32,
    );
    let asize = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        TABLE_ASIZE_OFFSET as i32,
    );
    let avals_bytes = bcx.ins().ishl_imm_u(asize, 3);
    let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
    let tag_addr = bcx.ins().iadd(atags_ptr, key_minus_1);
    let tag = bcx
        .ins()
        .uload8(types::I64, MemFlagsData::trusted(), tag_addr, 0);
    let tag_ok = bcx.ins().icmp_imm_u(IntCC::Equal, tag, want);
    let val_off = bcx.ins().ishl_imm_u(key_minus_1, 3);
    let val_addr = bcx.ins().iadd(avals_ptr, val_off);
    let fast_bits = bcx
        .ins()
        .load(types::I64, MemFlagsData::trusted(), val_addr, 0);
    bcx.ins().brif(
        tag_ok,
        merge_blk,
        &[BlockArg::Value(fast_bits)],
        deopt_blk,
        &[],
    );

    bcx.switch_to_block(slow_blk);
    bcx.seal_block(slow_blk);
    let (helper, key) = slow;
    let slot = bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let out = bcx.ins().stack_addr(types::I64, slot, 0);
    let mut sig = module.make_signature();
    for _ in 0..4 {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function(helper, Linkage::Import, &sig)
        .ok()?;
    let f = module.declare_func_in_func(id, bcx.func);
    let want_v = bcx.ins().iconst(types::I64, want);
    let call = bcx.ins().call(f, &[t, key, want_v, out]);
    let ok = bcx.inst_results(call)[0];
    let slow_bits = bcx.ins().stack_load(types::I64, types::I64, slot, 0);
    bcx.ins()
        .brif(ok, merge_blk, &[BlockArg::Value(slow_bits)], deopt_blk, &[]);

    bcx.switch_to_block(deopt_blk);
    bcx.seal_block(deopt_blk);
    let park_sig = module.make_signature();
    let park_id = module
        .declare_function("luna_jit_park_deopt", Linkage::Import, &park_sig)
        .ok()?;
    let park = module.declare_func_in_func(park_id, bcx.func);
    bcx.ins().call(park, &[]);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);

    bcx.switch_to_block(merge_blk);
    bcx.seal_block(merge_blk);
    Some(bcx.block_params(merge_blk)[0])
}
/// align a value with the Variable's declared Cranelift type
/// before def_var. The scan should have pinned every register's kind
/// tightly; this acts as a safety net so a slipped Unset register
/// (rare, e.g. a register whose only writer is on a path the BFS
/// didn't visit because of a Call wall) doesn't trip the
/// "declared type mismatch" verifier. Real type errors still bail
/// upstream — bitcast i64↔f64 is well-defined for any bit-pattern.
#[inline]
pub(super) fn aligned_def(
    bcx: &mut FunctionBuilder<'_>,
    regs: &[Variable],
    kinds: &[RegKind],
    idx: usize,
    value: Value,
) {
    let want = match kinds.get(idx).copied().unwrap_or(RegKind::Unset) {
        RegKind::Float => types::F64,
        RegKind::Int | RegKind::Unset | RegKind::Table => types::I64,
    };
    let got = bcx.func.dfg.value_type(value);
    let aligned = if got == want {
        value
    } else {
        bcx.ins().bitcast(want, MemFlagsData::new(), value)
    };
    bcx.def_var(regs[idx], aligned);
}

#[inline]
pub(super) fn jmp_target(pc: usize, inst: Inst) -> usize {
    // PUC `Jmp`: pc += sJ (after the Jmp is advanced past). New PC =
    // (pc + 1) + sj. Cast carefully — backward jumps would underflow
    // a plain usize add but our forward-only whitelist keeps them out.
    let new_pc = pc as i64 + 1 + inst.sj() as i64;
    new_pc as usize
}

/// The kind `k` gives register `idx`, Int past its end.
#[inline]
pub(super) fn a_kind(k: &[RegKind], idx: u32) -> RegKind {
    k.get(idx as usize).copied().unwrap_or(RegKind::Int)
}

/// Whether array index `key_minus_1` falls in `t`'s array part, as an
/// `i8`: below `alimit`, or below the array size, which then raises
/// `alimit` to the key (a 5.4 table after `#t` lowered it), as
/// `Table::array_index` does.
pub(super) fn emit_array_in_range(
    bcx: &mut FunctionBuilder<'_>,
    t: Value,
    key_minus_1: Value,
) -> Value {
    let flags = MemFlagsData::trusted();
    let len_flags = crate::jit_backend::trace::len_state_flags_in(bcx.func);
    let alimit = bcx
        .ins()
        .load(types::I32, len_flags, t, TABLE_ALIMIT_OFFSET);
    let alimit = bcx.ins().uextend(types::I64, alimit);
    let in_limit = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, alimit);
    let past_blk = bcx.create_block();
    let raise_blk = bcx.create_block();
    let merge_blk = bcx.create_block();
    bcx.append_block_param(merge_blk, types::I8);
    bcx.ins().brif(
        in_limit,
        merge_blk,
        &[BlockArg::Value(in_limit)],
        past_blk,
        &[],
    );
    bcx.switch_to_block(past_blk);
    bcx.seal_block(past_blk);
    let asize = bcx
        .ins()
        .load(types::I64, flags, t, TABLE_ASIZE_OFFSET as i32);
    let in_array = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
    bcx.ins().brif(
        in_array,
        raise_blk,
        &[],
        merge_blk,
        &[BlockArg::Value(in_array)],
    );
    bcx.switch_to_block(raise_blk);
    bcx.seal_block(raise_blk);
    let key = bcx.ins().iadd_imm_u(key_minus_1, 1);
    let key32 = bcx.ins().ireduce(types::I32, key);
    bcx.ins().store(len_flags, key32, t, TABLE_ALIMIT_OFFSET);
    bcx.ins().jump(merge_blk, &[BlockArg::Value(in_array)]);
    bcx.switch_to_block(merge_blk);
    bcx.seal_block(merge_blk);
    bcx.block_params(merge_blk)[0]
}

/// A numeric `for`'s registers once its `ForPrep` has run: the index, the
/// count (a float loop: the limit), the step, and the loop variable, which
/// in 5.5's layout is the index itself.
#[derive(Clone, Copy)]
pub(super) struct ForRegs {
    pub idx: usize,
    pub x: usize,
    pub step: usize,
    pub var: usize,
}

impl ForRegs {
    pub(super) fn of(ins: Inst) -> ForRegs {
        let a = ins.a() as usize;
        if matches!(ins.op(), Op::ForPrep55 | Op::ForLoop55) {
            ForRegs {
                idx: a + 2,
                x: a,
                step: a + 1,
                var: a + 2,
            }
        } else {
            ForRegs {
                idx: a,
                x: a + 1,
                step: a + 2,
                var: a + 3,
            }
        }
    }
}
