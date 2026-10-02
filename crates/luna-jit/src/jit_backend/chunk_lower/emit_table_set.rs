use super::*;

pub(super) fn emit_new_table<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds,
        presize_for_newtable,
        regs,
        ..
    } = f;
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::NewTable => {
            // `R[A] = {}` lowers to a call into the
            // `luna_jit_new_table` Rust helper. The helper reads
            // the active Vm pointer from the thread-local set by
            // `enter_jit`. Result is the `Gc<Table>` pointer
            // pun'd to I64, written into R[A].
            //
            // when the scan recorded a presize hint
            // (the NewTable opens a counted `for i = 1, N`
            // window), reach for the `_sized` variant with N
            // as an i64 const arg. Skips the O(log N) rehash
            // chain that would otherwise dominate the loop.
            //
            // also honour `NewTable.B` as a presize
            // hint: luna's frontend emits `NewTable A B=N` for
            // `{a, b, c, ...}` literals (the SetList that
            // follows fills exactly N entries). Either source —
            // the scanned window or NewTable.B — feeds the sized
            // helper; the explicit window wins on overlap.
            let presize = presize_for_newtable.get(&pc).copied().or_else(|| {
                let b = ins.b();
                if b > 0 { Some(b as i64) } else { None }
            });
            let g = if let Some(n) = presize {
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_new_table_sized", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let n_v = bcx.ins().iconst(types::I64, n);
                let call_inst = bcx.ins().call(r, &[n_v]);
                bcx.inst_results(call_inst)[0]
            } else {
                let mut sig = module.make_signature();
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_new_table", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call_inst = bcx.ins().call(r, &[]);
                bcx.inst_results(call_inst)[0]
            };
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, g);
            current_kinds[ins.a() as usize] = RegKind::Table;
            current_is_nil[ins.a() as usize] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

pub(super) fn emit_set_table<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    f: EmitFacts<'_>,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds, regs, ..
    } = f;
    match ins.op() {
        Op::SetTable => {
            // `R[A][R[B]] = R[C]`. Pick the Int/Int vs
            // Float/Float helper at emit time based on R[B]'s
            // resolved kind (the scan pinned R[B] and R[C] to
            // the same kind).
            //
            // for the Int/Int variant, emit an inline
            // aset fast path: skip the helper call when the key
            // falls inside the table's array part. The cranelift
            // IR reads `atags.len`, `atags.ptr`, `avals.ptr`
            // straight from the `Gc<Table>` raw ptr (`#[repr(C)]`
            // + `offset_of!` make the layout stable), branches on
            // `(key - 1) as u64 < atags.len`, and either writes
            // the tag byte + i64 payload in-place or falls
            // through to the slow-path helper.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            let t_raw = bcx.use_var(regs[a]);
            // when `R[A]` is Float-declared
            // because of a same-slot Float writer in another BB
            // (the binary_trees 5.1/5.2 pattern), `use_var` hands
            // back F64. Bitcast back to I64 so the inline aset
            // load / helper call sees a real `Gc<Table>` ptr.
            // Lossless reinterpret — `aligned_def` did the
            // matching F64→I64 bitcast at the NewTable write.
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
                // Inline aset fast path (Int key + Int val).
                // load `asize` (u64) once for both the
                // in-range check and the `atags_ptr = avals_ptr +
                // asize * 8` computation. Avals occupy `slab` from
                // offset 0; atags trail at byte offset `asize * 8`.
                let asize = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let one = bcx.ins().iconst(types::I64, 1);
                let key_minus_1 = bcx.ins().isub(key, one);
                // `(key - 1) as u64 < asize` handles both
                // `key >= 1` (else underflow → > any len) and
                // `key <= asize` in one unsigned compare.
                let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);

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
                let id = module
                    .declare_function("luna_jit_table_set_int", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let _ = bcx.ins().call(r, &[t, key, val]);
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
                let id = module
                    .declare_function("luna_jit_table_set_float_float", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let _ = bcx.ins().call(r, &[t, key_i, val_i]);
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
            // Float-declared Table operand
            // bitcast to I64; see SetTable for the rationale.
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
            let asize = bcx.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                t,
                TABLE_ASIZE_OFFSET as i32,
            );
            let b_v = bcx.ins().iconst(types::I64, b as i64);
            let fits = bcx
                .ins()
                .icmp(IntCC::UnsignedGreaterThanOrEqual, asize, b_v);
            // the inline stores fill an all-nil array part (a fresh
            // constructor table) with non-nil values, so afterwards
            // `acount` and `aprefix` are both `b`; anything else takes
            // the helper path, which keeps them itself
            let fits = if elems.iter().all(|&(tag, _)| tag != RAW_TAG_NIL) {
                let acount =
                    bcx.ins()
                        .load(types::I32, MemFlagsData::trusted(), t, TABLE_ACOUNT_OFFSET);
                let empty = bcx.ins().icmp_imm_u(IntCC::Equal, acount, 0);
                bcx.ins().band(fits, empty)
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
            let id = module
                .declare_function("luna_jit_table_set_raw", Linkage::Import, &sig)
                .ok()?;
            let r = module.declare_func_in_func(id, bcx.func);
            for (i, &(tag, bits)) in elems.iter().enumerate() {
                let key = bcx.ins().iconst(types::I64, i as i64 + 1);
                let tag_v = bcx.ins().iconst(types::I64, tag);
                let _ = bcx.ins().call(r, &[t, key, bits, tag_v]);
            }
            bcx.ins().jump(merge_blk, &[]);

            bcx.switch_to_block(merge_blk);
            bcx.seal_block(merge_blk);
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
