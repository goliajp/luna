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

pub(super) fn emit_set_list<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        c: ChunkIn { code, .. },
        reg_kinds,
        regs,
        ..
    } = f;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::SetList => {
            // `R[A][1..=B] = R[A+1..A+B]`. Each store goes inline
            // through the atags/avals layout the SetTable fast path
            // uses when the array part holds all B slots; the table
            // is normally the preceding `NewTable` presized to B, but
            // that is not proven here, so a smaller array part takes
            // the helper path, which grows the table. Each element's
            // tag is picked at emit time from `RegKind[A+i]`:
            //   Int     → raw::INT     (i64 verbatim)
            //   Float   → raw::FLOAT   (bitcast f64 → i64)
            //   Table   → raw::TABLE   (i64 ptr verbatim)
            //
            // `B == 0` variadic form: the matching
            // preceding `Op::Call C=0` returns exactly 1 value
            // (the self-recursive callee's `returns_one == true`
            // guarantee), so the static count is
            // `A_call - A_list`. Source regs are still
            // `R[A+1..A+count]`.
            let a = ins.a() as usize;
            let b_field = ins.b();
            let b = if b_field == 0 {
                let prev = code[pc - 1];
                (prev.a() as usize).saturating_sub(a)
            } else {
                b_field as usize
            };
            let t_raw = bcx.use_var(regs[a]);
            // a Float-declared table operand: bitcast to I64, as SetTable does
            let t = if matches!(
                reg_kinds.get(a).copied().unwrap_or(RegKind::Int),
                RegKind::Float
            ) {
                bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
            } else {
                t_raw
            };
            let mut elems = Vec::with_capacity(b);
            for i in 0..b {
                let src = a + 1 + i;
                let v = bcx.use_var(regs[src]);
                // per-PC kind from `current_kinds`,
                // not the global `reg_kinds`. R[A+i] may legitimately
                // hold an Int at one SetList PC and a Table at
                // another (the binary_trees `make` pattern).
                let kind = current_kinds.get(src).copied().unwrap_or(RegKind::Int);
                // collapse to I64 first
                // (lossless when declared F64), then pick the
                // tag. Handles all (declared × active) ∈ {F64,
                // I64} × {Int, Float, Table} correctly: a
                // Float-declared slot whose active kind here is
                // Int or Table still stores its 8-byte payload
                // verbatim under the right tag.
                let is_nil_src = current_is_nil.get(src).copied().unwrap_or(false);
                let (tag, bits) = if is_nil_src {
                    // slot was last written by LoadNil
                    // in this BB; store the Nil tag + 0 bits so
                    // `t[i] = nil`. Without this an `if t[i] ==
                    // nil` check would see `Int(0)` and miscompile.
                    let zero = bcx.ins().iconst(types::I64, 0);
                    (RAW_TAG_NIL, zero)
                } else {
                    let bits = if matches!(
                        reg_kinds.get(src).copied().unwrap_or(RegKind::Int),
                        RegKind::Float
                    ) {
                        bcx.ins().bitcast(types::I64, MemFlagsData::new(), v)
                    } else {
                        v
                    };
                    let tag = match kind {
                        RegKind::Int | RegKind::Unset => RAW_TAG_INT,
                        RegKind::Float => RAW_TAG_FLOAT,
                        RegKind::Table => RAW_TAG_TABLE,
                    };
                    (tag, bits)
                };
                elems.push((tag, bits));
            }
            // the array part first grows to exactly `b` slots when it is
            // smaller (the open form's call result is not counted in the
            // NewTable's size), as the interpreter's SetList does
            let asize = bcx
                .ins()
                .uload32(MemFlagsData::trusted(), t, TABLE_ASIZE_OFFSET as i32);
            let b_v = bcx.ins().iconst(types::I64, b as i64);
            let short = bcx.ins().icmp(IntCC::UnsignedLessThan, asize, b_v);
            let grow_blk = bcx.create_block();
            let sized_blk = bcx.create_block();
            bcx.ins().brif(short, grow_blk, &[], sized_blk, &[]);
            bcx.switch_to_block(grow_blk);
            bcx.seal_block(grow_blk);
            let mut grow_sig = module.make_signature();
            grow_sig.params.push(AbiParam::new(types::I64));
            grow_sig.params.push(AbiParam::new(types::I64));
            let grow_id = module
                .declare_function("luna_jit_table_reserve_list", Linkage::Import, &grow_sig)
                .ok()?;
            let grow_ref = module.declare_func_in_func(grow_id, bcx.func);
            bcx.ins().call(grow_ref, &[t, b_v]);
            bcx.ins().jump(sized_blk, &[]);
            bcx.switch_to_block(sized_blk);
            bcx.seal_block(sized_blk);
            let asize = bcx
                .ins()
                .uload32(MemFlagsData::trusted(), t, TABLE_ASIZE_OFFSET as i32);
            // the inline stores fill an all-nil array part (a fresh
            // constructor table) with non-nil values, so afterwards
            // `acount` and `aprefix` are both `b`; anything else takes
            // the helper path, which keeps them itself
            let fits = if elems.iter().all(|&(tag, _)| tag != RAW_TAG_NIL) {
                let acount =
                    bcx.ins()
                        .load(types::I32, MemFlagsData::trusted(), t, TABLE_ACOUNT_OFFSET);
                let empty = bcx.ins().icmp_imm_u(IntCC::Equal, acount, 0);
                // a growth that failed parked an error and left it short
                let room = bcx
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, asize, b_v);
                let fits = bcx.ins().band(room, empty);
                if elems.iter().any(|&(tag, _)| tag == RAW_TAG_TABLE) {
                    // a black table takes the write barrier, on the slow path
                    use luna_core::runtime::table::jit_layout::{
                        TABLE_FLAGS_OFFSET, TABLE_SLOW_MASK,
                    };
                    let flags = bcx.ins().uload8(
                        types::I64,
                        MemFlagsData::trusted(),
                        t,
                        TABLE_FLAGS_OFFSET as i32,
                    );
                    let slow = bcx.ins().band_imm_u(flags, i64::from(TABLE_SLOW_MASK));
                    let plain = bcx.ins().icmp_imm_u(IntCC::Equal, slow, 0);
                    bcx.ins().band(fits, plain)
                } else {
                    fits
                }
            } else {
                bcx.ins().iconst(types::I8, 0)
            };
            let fast_blk = bcx.create_block();
            let slow_blk = bcx.create_block();
            let merge_blk = bcx.create_block();
            bcx.ins().brif(fits, fast_blk, &[], slow_blk, &[]);

            bcx.switch_to_block(fast_blk);
            bcx.seal_block(fast_blk);
            // `atags_ptr = avals_ptr + asize * 8`, once for the
            // whole literal
            let avals_ptr = bcx.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                t,
                TABLE_ARRAY_PTR_OFFSET as i32,
            );
            let three_imm = bcx.ins().iconst(types::I64, 3);
            let avals_bytes = bcx.ins().ishl(asize, three_imm);
            let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
            for (i, &(tag, bits)) in elems.iter().enumerate() {
                let idx_const = bcx.ins().iconst(types::I64, i as i64);
                let tag_dst = bcx.ins().iadd(atags_ptr, idx_const);
                let tag_byte = bcx.ins().iconst(types::I8, tag);
                bcx.ins()
                    .store(MemFlagsData::trusted(), tag_byte, tag_dst, 0);
                let val_off = bcx.ins().iconst(types::I64, (i as i64) * 8);
                let val_dst = bcx.ins().iadd(avals_ptr, val_off);
                bcx.ins().store(MemFlagsData::trusted(), bits, val_dst, 0);
            }
            let filled = bcx.ins().iconst(types::I32, b as i64);
            bcx.ins()
                .store(MemFlagsData::trusted(), filled, t, TABLE_ACOUNT_OFFSET);
            bcx.ins()
                .store(MemFlagsData::trusted(), filled, t, TABLE_APREFIX_OFFSET);
            bcx.ins().jump(merge_blk, &[]);

            bcx.switch_to_block(slow_blk);
            bcx.seal_block(slow_blk);
            let mut sig = module.make_signature();
            for _ in 0..4 {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let id = module
                .declare_function("luna_jit_table_set_raw", Linkage::Import, &sig)
                .ok()?;
            let r = module.declare_func_in_func(id, bcx.func);
            for (i, &(tag, bits)) in elems.iter().enumerate() {
                let key = bcx.ins().iconst(types::I64, i as i64 + 1);
                let tag_v = bcx.ins().iconst(types::I64, tag);
                let call = bcx.ins().call(r, &[t, key, bits, tag_v]);
                let stored = bcx.inst_results(call)[0];
                emit_hand_if_zero(module, bcx, st, f, pc, stored)?;
            }
            bcx.ins().jump(merge_blk, &[]);

            bcx.switch_to_block(merge_blk);
            bcx.seal_block(merge_blk);
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
