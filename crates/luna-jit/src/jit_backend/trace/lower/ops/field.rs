use super::*;

/// String-key reads.
pub(super) fn emit_get_field_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record,
        head_proto,
        opts,
        effective_end,
        ..
    } = *pl;
    let OpHelpers {
        get_field_id,
        get_field_checked_id,
        ..
    } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::GetField => {
            // sunk path: use_var the virt slot
            // for hash_slot, def_var R[A], propagate kind.
            if let Some(OpAction::GetFieldSunkRead {
                site_idx,
                hash_slot,
            }) = lw.escape.op_actions[i]
                && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
                && let Some(vars) = lw.virt_vars[site_idx as usize].as_ref()
            {
                let array_cap = lw.escape.sites[site_idx as usize].array_cap as usize;
                let slot = array_cap + hash_slot as usize;
                let v = lw.bcx.use_var(vars[slot]);
                lw.bcx.def_var(regs[ins.a() as usize], v);
                let k = lw.virt_kinds[site_idx as usize]
                    .as_ref()
                    .expect("Sinkable site has virt_kinds")[slot];
                lw.current_kinds[off + ins.a() as usize] = k;
                return Some(());
            }
            // helper path.
            // the helpers read the operand as a table. A number,
            // string or closure (entry-guarded or computed here) leaves
            // the op to the interpreter; Nil can be a lookahead guess
            // for a value the recording indexed, so it stays
            match k_op(&lw.current_kinds, off as u32 + ins.b()) {
                RegKind::Table | RegKind::Nil => {}
                RegKind::Unset | RegKind::Unknown => {
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
                }
                _ => return None,
            }
            let t = lw.bcx.use_var(regs[ins.b() as usize]);
            let key_v = match head_proto.consts[ins.c() as usize] {
                luna_core::runtime::Value::Str(s) => s,
                _ => unreachable!("pre-emit gates Str const at K[C]"),
            };
            let key_arg = emit_str_key_arg(&mut lw.bcx, key_v, opts.aot, &mut lw.defined_aot_data);
            let inferred = infer_getx_exit(record, i, effective_end);
            let want = getx_want(inferred);

            // table-field IC scaffold.
            //
            // When `LUNA_JIT_FIELD_IC=1` and this op is the
            // recorder-captured snapshot site, emit an inline
            // cache: 4 guards (mt None, nodes.len() == cached,
            // node[slot].key.raw == cached_key_bits,
            // node[slot].val.tag == cached_val_tag) + 1 load
            // of node[slot].val.raw. Guard miss falls through
            // to the existing helper-call path so no new deopt
            // edge is introduced (scaffold-safe rollout).
            //
            // env-OFF default short-circuits on the cached
            // atomic load inside `field_ic_enabled()`; the IC
            // emission produces zero additional IR when the
            // gate is off.
            // The IC's tag guard only stands in for the checked read
            // when the cached tag is the one the trace types it as.
            let ic_active = luna_core::jit::trace_types::field_ic_enabled()
                && record.field_ic_snapshot.as_ref().is_some_and(|s| {
                    s.op_idx as usize == i
                        && want.is_none_or(|(k, _)| {
                            use luna_core::runtime::value::tag;
                            let enum_tag = match k {
                                RegKind::Int => tag::INT,
                                RegKind::Float => tag::FLOAT,
                                _ => tag::TABLE,
                            };
                            enum_tag == s.cached_val_tag
                        })
                });

            let v = if ic_active {
                emit_field_ic_read(lw, pl, oc, t, key_arg, want)
            } else if let (Some(slot), Some((_, w))) = (record.field_slot(i), want) {
                emit_field_slot_read(lw, pl, oc, t, key_arg, slot, w)
            } else if let Some((_, w)) = want {
                checked_read!(lw, pl, get_field_checked_id, t, key_arg, w, rop.pc, i)
            } else {
                let func_ref = lw.bcx.import_func(get_field_id);
                let call = lw.bcx.ins().call(func_ref, &[t, key_arg]);
                lw.bcx.inst_results(call)[0]
            };
            lw.bcx.def_var(regs[ins.a() as usize], v);

            match getx_want(inferred) {
                Some((kind, _)) => lw.current_kinds[off + ins.a() as usize] = kind,
                None => {
                    // as for GetI
                    lw.current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("GetField:inference-fail"));
                }
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// A field read straight from the hash slot the key was recorded in; the
/// trace leaves for the interpreter when the slot no longer holds it (see
/// `array_read`).
#[allow(clippy::too_many_arguments)]
fn emit_field_slot_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    t: Value,
    key_arg: Value,
    slot: u32,
    w: u8,
) -> Value {
    let OpCx { i, rop, .. } = *oc;
    let hit = lw.bcx.create_block();
    lw.bcx.append_block_param(hit, types::I64);
    let miss = lw.bcx.create_block();
    let merge = lw.bcx.create_block();
    lw.bcx.append_block_param(merge, types::I64);
    field_slot::emit_field_slot_check(&mut lw.bcx, t, key_arg, slot, Some(w), hit, miss);
    lw.bcx.switch_to_block(hit);
    lw.bcx.seal_block(hit);
    let node = lw.bcx.block_params(hit)[0];
    let fast = field_slot::emit_slot_load(&mut lw.bcx, node);
    lw.bcx.ins().jump(merge, &[fast.into()]);
    lw.bcx.switch_to_block(miss);
    lw.bcx.seal_block(miss);
    guard_exit(lw, pl, rop.pc, i);
    lw.bcx.switch_to_block(merge);
    lw.bcx.seal_block(merge);
    lw.bcx.block_params(merge)[0]
}

/// Global reads through an upvalue table.
pub(super) fn emit_get_tab_up_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record,
        head_proto,
        opts,
        effective_end,
        ..
    } = *pl;
    let OpHelpers {
        get_tab_up_id,
        get_tab_up_checked_id,
        ..
    } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::GetTabUp => {
            // `R[A] := upvals[B][K[C]:string]`.
            // Helper path mirrors GetField's; the sunk-table
            // optimization does NOT apply (upvalue tables are
            // the global env, not trace-internal alloc). Exit-tag
            // inference identical to GetField — peek next op via
            // `infer_getx_exit`.
            let upval_idx_arg = lw.bcx.ins().iconst(types::I64, ins.b() as i64);
            let key_v = match head_proto.consts[ins.c() as usize] {
                luna_core::runtime::Value::Str(s) => s,
                _ => unreachable!("pre-emit gates Str const at K[C]"),
            };
            let key_arg = emit_str_key_arg(&mut lw.bcx, key_v, opts.aot, &mut lw.defined_aot_data);
            let inferred = infer_getx_exit(record, i, effective_end);
            let v = if let Some((_, w)) = getx_want(inferred) {
                checked_read!(
                    lw,
                    pl,
                    get_tab_up_checked_id,
                    upval_idx_arg,
                    key_arg,
                    w,
                    rop.pc,
                    i
                )
            } else {
                let func_ref = lw.bcx.import_func(get_tab_up_id);
                let call = lw.bcx.ins().call(func_ref, &[upval_idx_arg, key_arg]);
                lw.bcx.inst_results(call)[0]
            };
            lw.bcx.def_var(regs[ins.a() as usize], v);
            match getx_want(inferred) {
                Some((kind, _)) => lw.current_kinds[off + ins.a() as usize] = kind,
                None => {
                    // as for GetI
                    lw.current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("GetTabUp:inference-fail"));
                }
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// The inline-cached read of `t[key]`: guarded load from the cached node, helper on a miss.
pub(super) fn emit_field_ic_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    t: Value,
    key_arg: Value,
    want: Option<(RegKind, u8)>,
) -> Value {
    let Plan { record, .. } = *pl;
    let OpHelpers {
        get_field_id,
        get_field_checked_id,
        ..
    } = lw.h.op;
    let OpCx { i, rop, .. } = *oc;
    let snap = record
        .field_ic_snapshot
        .as_ref()
        .expect("ic_active implies snapshot present");

    // --- Guards 1 & 2: metatable + node count ---
    let mt = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        t,
        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
    );
    let zero = lw.bcx.ins().iconst(types::I64, 0);
    let mt_ok = lw.bcx.ins().icmp(IntCC::Equal, mt, zero);
    let node_mask = lw.bcx.ins().load(
        types::I32,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        t,
        crate::jit_backend::TABLE_NODE_MASK_OFFSET as i32,
    );
    let mask = i64::from((snap.nodes_len as u32).wrapping_sub(1));
    let len_ok = lw.bcx.ins().icmp_imm_u(IntCC::Equal, node_mask, mask);
    let guards_12 = lw.bcx.ins().band(mt_ok, len_ok);

    // 3 blocks: fast (guards 3+4 + load), slow
    // (helper), merge (def_var dst). slow_blk has 2
    // predecessors (mt/len fail + key/tag fail); we
    // seal it only after both edges are emitted.
    let fast_blk = lw.bcx.create_block();
    let slow_blk = lw.bcx.create_block();
    let merge_blk = lw.bcx.create_block();
    lw.bcx.append_block_param(merge_blk, types::I64);

    lw.bcx.ins().brif(guards_12, fast_blk, &[], slow_blk, &[]);

    // --- fast: load nodes_ptr, compute node_addr,
    //     guards 3 & 4, load val_raw ---
    lw.bcx.switch_to_block(fast_blk);
    lw.bcx.seal_block(fast_blk);
    let nodes_ptr = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        t,
        crate::jit_backend::TABLE_NODES_PTR_OFFSET as i32,
    );
    let node_offset = (snap.slot_idx as usize * crate::jit_backend::SIZEOF_NODE) as i64;
    let node_addr = lw.bcx.ins().iadd_imm_u(nodes_ptr, node_offset);

    let key_raw = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        node_addr,
        crate::jit_backend::NODE_KEY_RAW_OFFSET as i32,
    );
    let key_imm = lw.bcx.ins().iconst(types::I64, snap.key_ptr_bits as i64);
    let key_ok = lw.bcx.ins().icmp(IntCC::Equal, key_raw, key_imm);

    let val_tag_i8 = lw.bcx.ins().load(
        types::I8,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        node_addr,
        crate::jit_backend::NODE_VAL_TAG_OFFSET as i32,
    );
    let val_tag = lw.bcx.ins().uextend(types::I64, val_tag_i8);
    let tag_imm = lw.bcx.ins().iconst(types::I64, snap.cached_val_tag as i64);
    let tag_ok = lw.bcx.ins().icmp(IntCC::Equal, val_tag, tag_imm);
    let guards_34 = lw.bcx.ins().band(key_ok, tag_ok);

    let load_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(guards_34, load_blk, &[], slow_blk, &[]);

    lw.bcx.switch_to_block(load_blk);
    lw.bcx.seal_block(load_blk);
    let val_raw = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        node_addr,
        crate::jit_backend::NODE_VAL_RAW_OFFSET as i32,
    );
    lw.bcx.ins().jump(merge_blk, &[val_raw.into()]);

    // --- slow: fall back to the helper ---
    lw.bcx.switch_to_block(slow_blk);
    lw.bcx.seal_block(slow_blk);
    let v_slow = if let Some((_, w)) = want {
        checked_read!(lw, pl, get_field_checked_id, t, key_arg, w, rop.pc, i)
    } else {
        let func_ref = lw.bcx.import_func(get_field_id);
        let call = lw.bcx.ins().call(func_ref, &[t, key_arg]);
        lw.bcx.inst_results(call)[0]
    };
    lw.bcx.ins().jump(merge_blk, &[v_slow.into()]);

    // --- merge ---
    lw.bcx.switch_to_block(merge_blk);
    lw.bcx.seal_block(merge_blk);
    lw.bcx.block_params(merge_blk)[0]
}
