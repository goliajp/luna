use super::*;

/// Table construction and integer-key reads.
pub(super) fn emit_table_new_get_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record,
        effective_end,
        ..
    } = *pl;
    let OpHelpers {
        new_table_id,
        get_field_checked_id,
        ..
    } = lw.h.op;
    let RuntimeHelpers { get_int_id, .. } = lw.h.rt;
    let OpCx { i, off, ins, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        Op::NewTable => {
            // sunk path: skip the heap alloc helper.
            // The site's virt slot Variables (allocated pre-emit)
            // hold the array elements directly. `current_kinds`
            // for the site's slot stays at its entry value
            // (Unset → maps to ExitTag::Untouched, so the
            // dispatcher carries the entry tag in the restore).
            if let Some(OpAction::NewTableSite { site_idx }) = lw.escape.op_actions[i]
                && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
                && lw.virt_vars[site_idx as usize].is_some()
            {
                return Some(());
            }
            let func_ref = lw.bcx.import_func(new_table_id);
            let call = lw.bcx.ins().call(func_ref, &[]);
            let t = lw.bcx.inst_results(call)[0];
            lw.bcx.def_var(regs[ins.a() as usize], t);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Table;
        }
        Op::GetI => {
            // sunk path: a GetI from a Sinkable site
            // at a key in `1..=cap` becomes a `use_var` of the
            // matching virt slot Variable, with kind carried
            // from `virt_kinds`.
            if let Some(OpAction::GetIRead { site_idx, key }) = lw.escape.op_actions[i]
                && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
                && let Some(vars) = lw.virt_vars[site_idx as usize].as_ref()
            {
                let slot = (key as usize) - 1;
                let v = lw.bcx.use_var(vars[slot]);
                lw.bcx.def_var(regs[ins.a() as usize], v);
                let k = lw.virt_kinds[site_idx as usize]
                    .as_ref()
                    .expect("Sinkable site has virt_kinds")[slot];
                lw.current_kinds[off + ins.a() as usize] = k;
                return Some(());
            }
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
            let k_imm = lw.bcx.ins().iconst(types::I64, ins.c() as i64);
            // GetX inference: look at the immediate next op. The read
            // is checked against it, so a value of another type (or
            // a table with a metatable) leaves the trace here.
            let inferred = infer_getx_exit(record, i, effective_end);
            if let Some((kind, want)) = getx_want(inferred) {
                let v = array_read(lw, pl, oc, t, k_imm, want);
                lw.bcx.def_var(regs[ins.a() as usize], v);
                lw.current_kinds[off + ins.a() as usize] = kind;
            } else {
                let func_ref = lw.bcx.import_func(get_int_id);
                let call = lw.bcx.ins().call(func_ref, &[t, k_imm]);
                let v = lw.bcx.inst_results(call)[0];
                lw.bcx.def_var(regs[ins.a() as usize], v);
                // the value's type is not known: the register's
                // earlier kind no longer describes it
                lw.current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                lw.dispatchable = false;
                lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("GetI:inference-fail"));
            }
        }
        Op::GetTable => {
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
            let key = lw.bcx.use_var(regs[ins.c() as usize]);
            let inferred = infer_getx_exit(record, i, effective_end);
            // the helper reads the key as an integer
            let key_is_int = matches!(k_op(&lw.current_kinds, off as u32 + ins.c()), RegKind::Int);
            let key_is_str = matches!(k_op(&lw.current_kinds, off as u32 + ins.c()), RegKind::Str);
            match getx_want(inferred) {
                Some((kind, want)) if key_is_int => {
                    let v = array_read(lw, pl, oc, t, key, want);
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    lw.current_kinds[off + ins.a() as usize] = kind;
                }
                // a string key reads as a field does
                Some((kind, want)) if key_is_str => {
                    let v = checked_read!(lw, pl, get_field_checked_id, t, key, want, oc.rop.pc, i);
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    lw.current_kinds[off + ins.a() as usize] = kind;
                }
                _ => {
                    let func_ref = lw.bcx.import_func(get_int_id);
                    let call = lw.bcx.ins().call(func_ref, &[t, key]);
                    let v = lw.bcx.inst_results(call)[0];
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    // as for GetI
                    lw.current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("GetTable:inference-fail"));
                }
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// Table stores.
pub(super) fn emit_table_set_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let OpHelpers { set_ids, .. } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        Op::SetField => {
            emit_set_field(lw, pl, oc)?;
        }
        Op::SetI => {
            // sunk path: when escape sweep tagged
            // SetISunkWrite, def_var the source register into
            // the matching virt slot Variable + propagate the
            // source RegKind into virt_kinds so the next
            // GetIRead restores the right kind into current_kinds.
            if let Some(OpAction::SetISunkWrite { site_idx, key }) = lw.escape.op_actions[i]
                && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
                && lw.virt_vars[site_idx as usize].is_some()
            {
                let slot = (key as usize) - 1;
                let src_kind = lw.current_kinds[off + ins.c() as usize];
                let v = lw.bcx.use_var(regs[ins.c() as usize]);
                let vars = lw.virt_vars[site_idx as usize]
                    .as_ref()
                    .expect("Sinkable site has virt_vars");
                lw.bcx.def_var(vars[slot], v);
                let kinds_vec = lw.virt_kinds[site_idx as usize]
                    .as_mut()
                    .expect("Sinkable site has virt_kinds");
                kinds_vec[slot] = src_kind;
                return Some(());
            }
            // R[A][B_imm] := R[C] helper path. Dispatch by R[C]
            // kind via emit_table_set (Nil / Int / Closure / etc.).
            // the helpers read the operand as a table. A number,
            // string or closure (entry-guarded or computed here) leaves
            // the op to the interpreter; Nil can be a lookahead guess
            // for a value the recording indexed, so it stays
            match k_op(&lw.current_kinds, off as u32 + ins.a()) {
                RegKind::Table | RegKind::Nil => {}
                RegKind::Unset | RegKind::Unknown => {
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
                }
                _ => return None,
            }
            let t = lw.bcx.use_var(regs[ins.a() as usize]);
            let k_imm = lw.bcx.ins().iconst(types::I64, ins.b() as i64);
            let val_kind = k_op(&lw.current_kinds, off as u32 + ins.c());
            // a value of unknown kind cannot be tagged for the table
            if val_kind.untyped() {
                return None;
            }
            let val = lw.bcx.use_var(regs[ins.c() as usize]);
            let stored_inline = array_write(&mut lw.bcx, t, k_imm, val, val_kind);
            let done = emit_table_set(&mut lw.bcx, &set_ids, t, k_imm, RegKind::Int, val, val_kind);
            guard!(lw, pl, done, i, rop.pc);
            array_write_join(&mut lw.bcx, stored_inline);
        }
        Op::SetTable => {
            // sunk path: escape sweep tagged
            // SetTableSunkWrite when the key reg was const-folded
            // to a 1..=cap literal. Emit shape mirrors SetI sunk
            // (def_var virt slot + propagate kind into virt_kinds).
            if let Some(OpAction::SetTableSunkWrite { site_idx, key }) = lw.escape.op_actions[i]
                && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
                && lw.virt_vars[site_idx as usize].is_some()
            {
                let slot = (key as usize) - 1;
                let src_kind = lw.current_kinds[off + ins.c() as usize];
                let v = lw.bcx.use_var(regs[ins.c() as usize]);
                let vars = lw.virt_vars[site_idx as usize]
                    .as_ref()
                    .expect("Sinkable site has virt_vars");
                lw.bcx.def_var(vars[slot], v);
                let kinds_vec = lw.virt_kinds[site_idx as usize]
                    .as_mut()
                    .expect("Sinkable site has virt_kinds");
                kinds_vec[slot] = src_kind;
                return Some(());
            }
            // R[A][R[B]] := R[C] helper path. Same kind-dispatch
            // as Op::SetI.
            // the helpers read the operand as a table. A number,
            // string or closure (entry-guarded or computed here) leaves
            // the op to the interpreter; Nil can be a lookahead guess
            // for a value the recording indexed, so it stays
            match k_op(&lw.current_kinds, off as u32 + ins.a()) {
                RegKind::Table | RegKind::Nil => {}
                RegKind::Unset | RegKind::Unknown => {
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
                }
                _ => return None,
            }
            let t = lw.bcx.use_var(regs[ins.a() as usize]);
            let key = lw.bcx.use_var(regs[ins.b() as usize]);
            let key_kind = k_op(&lw.current_kinds, off as u32 + ins.b());
            let val_kind = k_op(&lw.current_kinds, off as u32 + ins.c());
            // a key or value of unknown kind cannot be tagged for the table
            if key_kind.untyped() || val_kind.untyped() {
                return None;
            }
            let val = lw.bcx.use_var(regs[ins.c() as usize]);
            let stored_inline = if key_kind == RegKind::Int {
                array_write(&mut lw.bcx, t, key, val, val_kind)
            } else {
                None
            };
            let done = emit_table_set(&mut lw.bcx, &set_ids, t, key, key_kind, val, val_kind);
            guard!(lw, pl, done, i, rop.pc);
            array_write_join(&mut lw.bcx, stored_inline);
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// `Op::SetField`: into a sunk table's slot, or through the checked store helper.
pub(super) fn emit_set_field<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        head_proto, opts, ..
    } = *pl;
    let OpHelpers { set_ids, .. } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    // sunk path: when escape sweep tagged
    // SetFieldSunkWrite, def_var the source register into
    // the matching virt slot (array_cap + hash_slot) +
    // propagate the source RegKind into virt_kinds.
    if let Some(OpAction::SetFieldSunkWrite {
        site_idx,
        hash_slot,
    }) = lw.escape.op_actions[i]
        && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
        && lw.virt_vars[site_idx as usize].is_some()
    {
        let array_cap = lw.escape.sites[site_idx as usize].array_cap as usize;
        let slot = array_cap + hash_slot as usize;
        let src_kind = lw.current_kinds[off + ins.c() as usize];
        let v = lw.bcx.use_var(regs[ins.c() as usize]);
        let vars = lw.virt_vars[site_idx as usize]
            .as_ref()
            .expect("Sinkable site has virt_vars");
        lw.bcx.def_var(vars[slot], v);
        let kinds_vec = lw.virt_kinds[site_idx as usize]
            .as_mut()
            .expect("Sinkable site has virt_kinds");
        kinds_vec[slot] = src_kind;
        return Some(());
    }
    // helper path: R[A][K[B]:string] := R[C].
    // the helpers read the operand as a table. A number,
    // string or closure (entry-guarded or computed here) leaves
    // the op to the interpreter; Nil can be a lookahead guess
    // for a value the recording indexed, so it stays
    match k_op(&lw.current_kinds, off as u32 + ins.a()) {
        RegKind::Table | RegKind::Nil => {}
        RegKind::Unset | RegKind::Unknown => {
            lw.dispatchable = false;
            lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
        }
        _ => return None,
    }
    let t = lw.bcx.use_var(regs[ins.a() as usize]);
    let key_v = match head_proto.consts[ins.b() as usize] {
        luna_core::runtime::Value::Str(s) => s,
        _ => unreachable!("pre-emit gates Str const at K[B]"),
    };
    let key_arg = emit_str_key_arg(&mut lw.bcx, key_v, opts.aot, &mut lw.defined_aot_data);
    let val_kind = k_op(&lw.current_kinds, off as u32 + ins.c());
    // a value of unknown kind cannot be tagged for the table
    if val_kind.untyped() {
        return None;
    }
    let val = lw.bcx.use_var(regs[ins.c() as usize]);
    // a number overwriting a value already under the key goes
    // straight into its slot; anything else through the helper
    let slot = pl
        .record
        .field_slot(i)
        .filter(|_| matches!(val_kind, RegKind::Int | RegKind::Float));
    let merge = slot.map(|slot| {
        let bcx = &mut lw.bcx;
        let hit = bcx.create_block();
        bcx.append_block_param(hit, types::I64);
        let miss = bcx.create_block();
        let merge = bcx.create_block();
        field_slot::emit_field_slot_check(bcx, t, key_arg, slot, None, hit, miss);
        bcx.switch_to_block(hit);
        bcx.seal_block(hit);
        let node = bcx.block_params(hit)[0];
        field_slot::emit_slot_store(bcx, node, val, kind_tag(val_kind));
        bcx.ins().jump(merge, &[]);
        bcx.switch_to_block(miss);
        bcx.seal_block(miss);
        merge
    });
    let done = emit_table_set(
        &mut lw.bcx,
        &set_ids,
        t,
        key_arg,
        RegKind::Str,
        val,
        val_kind,
    );
    guard!(lw, pl, done, i, rop.pc);
    if let Some(merge) = merge {
        lw.bcx.ins().jump(merge, &[]);
        lw.bcx.switch_to_block(merge);
        lw.bcx.seal_block(merge);
    }
    Some(())
}
