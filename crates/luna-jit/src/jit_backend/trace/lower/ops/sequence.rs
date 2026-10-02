use super::*;

/// List stores, lengths and concatenation.
pub(super) fn emit_sequence_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan { record, .. } = *pl;
    let OpHelpers {
        set_ids,
        spill_id,
        stack_load_id,
        op_concat_id,
        ..
    } = lw.h.op;
    let RuntimeHelpers {
        update_raw_id,
        len_checked_id,
        ..
    } = lw.h.rt;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::SetList => {
            // `R[A][C+i] := R[A+i]` for i in
            // 1..=effective_b. effective_b = bytecode B if B>0,
            // else recorder's var_count snapshot (top - A - 1
            // at the op). For sunk path, effective_b == cap
            // (validated in escape sweep).
            let b_bytecode = ins.b() as usize;
            let effective_b = if b_bytecode == 0 {
                // Unreachable on the sunk path (the escape sweep
                // already mark_escaped on None). The helper path
                // bails compile too — None means no live top.
                record.ops[i].var_count? as usize
            } else {
                b_bytecode
            };
            if let Some(OpAction::SetListWrite { site_idx }) = lw.escape.op_actions[i]
                && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
                && lw.virt_vars[site_idx as usize].is_some()
            {
                let a = ins.a() as usize;
                let mut src_vals: Vec<Value> = Vec::with_capacity(effective_b);
                let mut src_kinds: Vec<RegKind> = Vec::with_capacity(effective_b);
                for vi in 1..=effective_b {
                    src_vals.push(lw.bcx.use_var(regs[a + vi]));
                    src_kinds.push(lw.current_kinds[off + a + vi]);
                }
                let vars = lw.virt_vars[site_idx as usize]
                    .as_ref()
                    .expect("Sinkable site has virt_vars");
                for (vi, &v) in src_vals.iter().enumerate() {
                    lw.bcx.def_var(vars[vi], v);
                }
                let kinds_vec = lw.virt_kinds[site_idx as usize]
                    .as_mut()
                    .expect("Sinkable site has virt_kinds");
                for (vi, &k) in src_kinds.iter().enumerate() {
                    kinds_vec[vi] = k;
                }
                return Some(());
            }
            // Helper path: same loop with effective_b iters.
            let a = ins.a() as usize;
            let c_off = ins.c() as i64;
            // the helpers read the operand as a table. A number,
            // string or closure (entry-guarded or computed here) leaves
            // the op to the interpreter; Nil can be a lookahead guess
            // for a value the recording indexed, so it stays
            match k_op(&lw.current_kinds, off as u32 + a as u32) {
                RegKind::Table | RegKind::Nil => {}
                RegKind::Unset | RegKind::Unknown => {
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
                }
                _ => return None,
            }
            let t = lw.bcx.use_var(regs[a]);
            for ii in 1..=effective_b {
                let key = lw.bcx.ins().iconst(types::I64, c_off + ii as i64);
                let src_kind = k_op(&lw.current_kinds, (off + a + ii) as u32);
                // a value of unknown kind cannot be tagged for the table
                if src_kind.untyped() {
                    return None;
                }
                let val = lw.bcx.use_var(regs[a + ii]);
                // Always stored: SetList fills the fresh table of a
                // constructor, which has no metatable, at integer keys.
                let _ = emit_table_set(&mut lw.bcx, &set_ids, t, key, RegKind::Int, val, src_kind);
            }
        }
        Op::Len => {
            // R[A] := #R[B] — call luna_jit_table_len(t) -> i64.
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
            let hit = lw.bcx.create_block();
            lw.bcx.append_block_param(hit, types::I64);
            let miss = lw.bcx.create_block();
            let merge = lw.bcx.create_block();
            lw.bcx.append_block_param(merge, types::I64);
            array_slot::emit_len_check(&mut lw.bcx, t, hit, miss);
            lw.bcx.switch_to_block(hit);
            lw.bcx.seal_block(hit);
            let fast = lw.bcx.block_params(hit)[0];
            lw.bcx.ins().jump(merge, &[fast.into()]);
            lw.bcx.switch_to_block(miss);
            lw.bcx.seal_block(miss);
            let func_ref = lw.bcx.import_func(len_checked_id);
            let call = lw.bcx.ins().call(func_ref, &[t]);
            let v = lw.bcx.inst_results(call)[0];
            // -1: the table has a metatable
            let ok = lw
                .bcx
                .ins()
                .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, v, 0);
            guard!(lw, pl, ok, i, rop.pc);
            lw.bcx.ins().jump(merge, &[v.into()]);
            lw.bcx.switch_to_block(merge);
            lw.bcx.seal_block(merge);
            let v = lw.bcx.block_params(merge)[0];
            lw.bcx.def_var(regs[ins.a() as usize], v);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
        }
        // N-operand concat via helper.
        Op::Concat => {
            let a_us = ins.a() as usize;
            let n_operands = ins.b() as usize;
            // Spill every operand slot to vm.stack so the
            // helper's concat_run can read them. For Unset
            // kinds (e.g. Str — RegKind doesn't carry Str)
            // call stack_update_raw which preserves the
            // existing tag and only refreshes the raw bits.
            let spill_ref = lw.bcx.import_func(spill_id);
            let update_raw_ref = lw.bcx.import_func(update_raw_id);
            for slot in a_us..(a_us + n_operands) {
                let k = lw.current_kinds[off + slot];
                let slot_arg = lw.bcx.ins().iconst(types::I64, slot as i64);
                let raw_arg = lw.bcx.use_var(regs[slot]);
                let tag_byte_opt = match k {
                    // an operand is read, so never held on the stack
                    RegKind::StackHeld => return None,
                    k => known_tag(k),
                };
                if let Some(tag_byte) = tag_byte_opt {
                    let tag_arg = lw.bcx.ins().iconst(types::I64, tag_byte as i64);
                    lw.bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
                } else {
                    lw.bcx.ins().call(update_raw_ref, &[slot_arg, raw_arg]);
                }
            }
            // Call helper.
            let a_arg = lw.bcx.ins().iconst(types::I64, a_us as i64);
            let n_arg = lw.bcx.ins().iconst(types::I64, n_operands as i64);
            let func_ref = lw.bcx.import_func(op_concat_id);
            let call_inst = lw.bcx.ins().call(func_ref, &[a_arg, n_arg]);
            let status = lw.bcx.inst_results(call_inst)[0];
            // -1: an error or `__concat`; the interpreter redoes the op
            let ok = lw.bcx.ins().icmp_imm_s(IntCC::Equal, status, 0);
            guard!(lw, pl, ok, i, rop.pc);
            // Reload regs[A] (= result Str) from vm.stack via
            // luna_jit_stack_load helper. The helper deopts on the
            // `__concat` path, so a result here is always a string.
            let stack_load_ref = lw.bcx.import_func(stack_load_id);
            let a_arg_reload = lw.bcx.ins().iconst(types::I64, a_us as i64);
            let reload_inst = lw.bcx.ins().call(stack_load_ref, &[a_arg_reload]);
            let result_raw = lw.bcx.inst_results(reload_inst)[0];
            lw.bcx.def_var(regs[a_us], result_raw);
            lw.current_kinds[off + a_us] = RegKind::Str;
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}
