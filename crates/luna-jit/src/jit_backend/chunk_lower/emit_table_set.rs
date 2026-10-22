use super::*;

pub(super) fn emit_set_table<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds, regs, ..
    } = f;
    match ins.op() {
        Op::SetTable => {
            // `R[A][R[B]] = R[C]`, R[B] and R[C] of one kind (the scan
            // pinned them). Int/Int stores inline when the key falls in
            // the array part (the `#[repr(C)]` table's layout read from
            // its pointer), else calls the helper.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            let t_raw = bcx.use_var(regs[a]);
            // `R[A]` Float-declared (a Float writer of the slot in another
            // block) comes back F64: bitcast to the table pointer, as
            // `aligned_def` did the other way at the NewTable write
            let t = if matches!(
                reg_kinds.get(a).copied().unwrap_or(RegKind::Int),
                RegKind::Float
            ) {
                bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
            } else {
                t_raw
            };
            let key = bcx.use_var(regs[b]);
            let val = bcx.use_var(regs[c]);
            let is_float = matches!(a_kind(reg_kinds, b as u32), RegKind::Float);

            if !is_float {
                // `asize` once, for the range check and for the tags,
                // which follow the values at `asize * 8`
                let asize =
                    bcx.ins()
                        .uload32(MemFlagsData::trusted(), t, TABLE_ASIZE_OFFSET as i32);
                let one = bcx.ins().iconst(types::I64, 1);
                let key_minus_1 = bcx.ins().isub(key, one);
                // `(key - 1) as u64 < len` unsigned also rejects `key < 1`
                let in_range = emit_array_in_range(bcx, t, key_minus_1);

                let fast_blk = bcx.create_block();
                let slow_blk = bcx.create_block();
                let merge_blk = bcx.create_block();
                bcx.ins().brif(in_range, fast_blk, &[], slow_blk, &[]);

                bcx.switch_to_block(fast_blk);
                bcx.seal_block(fast_blk);
                let avals_ptr = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_ARRAY_PTR_OFFSET as i32,
                );
                // atags_ptr = avals_ptr + asize * 8
                let three = bcx.ins().iconst(types::I64, 3);
                let avals_bytes = bcx.ins().ishl(asize, three);
                let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
                let tag_dst = bcx.ins().iadd(atags_ptr, key_minus_1);
                let old_tag = bcx
                    .ins()
                    .uload8(types::I64, MemFlagsData::trusted(), tag_dst, 0);
                let tag_byte = bcx.ins().iconst(types::I8, RAW_TAG_INT);
                bcx.ins()
                    .store(MemFlagsData::trusted(), tag_byte, tag_dst, 0);
                let val_off = bcx.ins().ishl(key_minus_1, three); // *8
                let val_dst = bcx.ins().iadd(avals_ptr, val_off);
                bcx.ins().store(MemFlagsData::trusted(), val, val_dst, 0);
                emit_array_fill_count(bcx, t, key_minus_1, old_tag, merge_blk);

                bcx.switch_to_block(slow_blk);
                bcx.seal_block(slow_blk);
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_table_set_int", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call = bcx.ins().call(r, &[t, key, val]);
                let stored = bcx.inst_results(call)[0];
                emit_hand_if_zero(module, bcx, st, f, pc, stored)?;
                bcx.ins().jump(merge_blk, &[]);

                bcx.switch_to_block(merge_blk);
                bcx.seal_block(merge_blk);
            } else {
                // Float/Float — keep the helper-call form. The
                // inline aset path stores raw Int tag + bits,
                // which would mis-normalise integral floats (PUC
                // semantics demand `t[1.0] = 1.0` lands in the
                // Int(1) array slot, not in a Float-tagged hash
                // entry); `Table::set` does the normalisation.
                let key_i = bcx.ins().bitcast(types::I64, MemFlagsData::new(), key);
                let val_i = bcx.ins().bitcast(types::I64, MemFlagsData::new(), val);
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_table_set_float_float", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call = bcx.ins().call(r, &[t, key_i, val_i]);
                let stored = bcx.inst_results(call)[0];
                emit_hand_if_zero(module, bcx, st, f, pc, stored)?;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
