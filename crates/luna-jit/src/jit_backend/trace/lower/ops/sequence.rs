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
        reserve_list_id,
        op_concat_id,
        ..
    } = lw.h.op;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
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
            // the array part first grows to the last index, as the
            // interpreter's SetList does; the stores then land in it
            let last = lw.bcx.ins().iconst(types::I64, c_off + effective_b as i64);
            let reserve = lw.bcx.import_func(reserve_list_id);
            lw.bcx.ins().call(reserve, &[t, last]);
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
        Op::Len => emit_len(lw, pl, oc)?,
        // N-operand concat via helper.
        Op::Concat => {
            // 5.1–5.3 name their destination: the operands from C fold
            // into R[C] and the result goes to R[A] too
            let (first, out) = ins.concat_operands();
            let a_us = first as usize;
            let n_operands = ins.b() as usize;
            // the operands as (tag, payload) words for the helper to store;
            // a tag of -1 keeps the one the stack slot has
            let ss = lw
                .bcx
                .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    (16 * n_operands) as u32,
                    3,
                ));
            for j in 0..n_operands {
                let k = lw.current_kinds[off + a_us + j];
                let raw = lw.bcx.use_var(regs[a_us + j]);
                let tag = match k {
                    // an operand is read, so never held on the stack
                    RegKind::StackHeld => return None,
                    k => emit_kind_tag(&mut lw.bcx, k, raw),
                };
                let tag = tag.unwrap_or_else(|| lw.bcx.ins().iconst(types::I64, -1));
                lw.bcx
                    .ins()
                    .stack_store(types::I64, tag, ss, (16 * j) as i32);
                lw.bcx
                    .ins()
                    .stack_store(types::I64, raw, ss, (16 * j + 8) as i32);
            }
            // an emergency collection may run: the operands are on the
            // stack then and the registers below them go as roots
            let roots = emit_ssa_roots(lw, i, off + a_us);
            // the other operands as the concat leaves them: a number among
            // them is a string then (`tostring` converts it in place), which
            // the stack holds alive as PUC's does. Bit j asks the helper for
            // operand j's payload, bit 63 for those from 63 on
            let reload: Vec<usize> = (1..n_operands)
                .filter(|&j| {
                    let k = lw.current_kinds[off + a_us + j];
                    matches!(k, RegKind::Int | RegKind::Float | RegKind::Unknown)
                        && a_us + j != out as usize
                })
                .collect();
            let mask = reload.iter().fold(0u64, |m, &j| m | 1 << j.min(63));
            let a_arg = lw.bcx.ins().iconst(types::I64, a_us as i64);
            let n_arg = lw.bcx.ins().iconst(types::I64, n_operands as i64);
            let ops = lw.bcx.ins().stack_addr(types::I64, ss, 0);
            let mask_arg = lw.bcx.ins().iconst(types::I64, mask as i64);
            let func_ref = lw.bcx.import_func(op_concat_id);
            let call_inst = lw
                .bcx
                .ins()
                .call(func_ref, &[a_arg, n_arg, ops, mask_arg, roots]);
            let result_raw = lw.bcx.inst_results(call_inst)[0];
            // 0: an error or `__concat`; the interpreter redoes the op
            let ok = lw.bcx.ins().icmp_imm_s(IntCC::NotEqual, result_raw, 0);
            guard!(lw, pl, ok, i, rop.pc);
            for r in [a_us, out as usize] {
                lw.bcx.def_var(regs[r], result_raw);
                lw.current_kinds[off + r] = RegKind::Str;
            }
            for j in reload {
                let slot = a_us + j;
                let raw = lw
                    .bcx
                    .ins()
                    .stack_load(types::I64, types::I64, ss, (16 * j + 8) as i32);
                lw.bcx.def_var(regs[slot], raw);
                if lw.current_kinds[off + slot] != RegKind::Unknown {
                    lw.current_kinds[off + slot] = RegKind::Str;
                }
            }
            gc_due_exit(lw, pl, i, rop.pc + 1);
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// `R[A] := #R[B]`: the array part's border inline, else
/// `luna_jit_table_len_checked`.
fn emit_len<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    let len_checked_id = lw.h.rt.len_checked_id;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    // R[A] := #R[B] — call luna_jit_table_len(t) -> i64.
    // the helpers read the operand as a table. A number,
    // string or closure (entry-guarded or computed here) leaves
    // the op to the interpreter; Nil can be a lookahead guess
    // for a value the recording indexed, so it stays
    match k_op(&lw.current_kinds, off as u32 + ins.b()) {
        RegKind::Table | RegKind::Nil => {}
        // a string's length is its header's, with no metamethod in any
        // dialect
        RegKind::Str => {
            let s = lw.bcx.use_var(regs[ins.b() as usize]);
            let n = lw.bcx.ins().load(
                types::I32,
                MemFlagsData::trusted(),
                s,
                crate::jit_backend::STR_LEN_OFFSET as i32,
            );
            let n = lw.bcx.ins().uextend(types::I64, n);
            lw.bcx.def_var(regs[ins.a() as usize], n);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
            return Some(());
        }
        RegKind::Unset | RegKind::Unknown => {
            lw.dispatchable = false;
            lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
        }
        _ => return None,
    }
    let t = lw.bcx.use_var(regs[ins.b() as usize]);
    let hit = lw.bcx.create_block();
    lw.bcx.append_block_param(hit, types::I64);
    let miss = lw.bcx.create_block();
    let merge = lw.bcx.create_block();
    lw.bcx.append_block_param(merge, types::I64);
    let rules = array_slot::TableRules::of(pl.opts.dialect);
    array_slot::emit_len_check(&mut lw.bcx, rules, t, hit, miss);
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
    Some(())
}
