use super::*;

/// The generic-for iterator call.
pub(super) fn emit_tfor_call_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let OpHelpers { spill_id, .. } = lw.h.op;
    let OpCx { off, ins, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        // generic-for body tail. Sequence:
        //   1. Spill regs[A..=A+2] (iter / state / control) to
        //      vm.stack so the helper's
        //      `vm.stack[A+4..=A+6] = vm.stack[A..=A+2]` copy
        //      sees current trace values (control changes each
        //      iter via TForLoop's R[A+2] = R[A+4] writeback).
        //   2. Call `luna_jit_op_tforcall(A, nvars)`. Status
        //      `< 0` → deopt (Lua-closure iter or runtime err).
        //   3. Continue branch: reload regs[A+2] + regs[A+4..]
        //      from vm.stack so subsequent body iters (after the
        //      back-edge from TForLoop) see iter results.
        //      current_kinds for reloaded slots = Unset; the
        //      first body iter still uses entry-tag kinds, and
        //      TForLoop tail's tag-check guards the back-edge
        //      so runtime types match emit-time assumptions.
        Op::TForCall => {
            let a_us = ins.a() as usize;
            let nvars = ins.c() as i64;
            // ipairs detection. Recorder's TForLoop
            // trigger snapshots `R[A]` if Native; we compare against
            // `ipairs_iter`'s address to specialise emit into inline
            // Table aget IR (skip the `op_tforcall` C call entirely
            // on the hot path).
            let ipairs_addr = luna_core::vm::builtins::ipairs_iter
                as luna_core::runtime::value::NativeFn as usize;
            let is_ipairs_trace = record.tfor_iter_ptr == Some(ipairs_addr);

            // spill discipline:
            // - non-ipairs case: spill R[A..=A+2] upfront (helper
            //   path runs unconditionally; needs vm.stack populated).
            // - ipairs case: SKIP the upfront spill on the hot
            //   path (R[A] and R[A+1] never change inside the
            //   trace — vm.stack still holds entry values, which
            //   is what the slow_blk helper reads). R[A+2] is
            //   spilled INSIDE slow_blk only, so fast iters pay
            //   nothing.
            let spill_ref = lw.bcx.import_func(spill_id);
            // a copy: the guard exits below take `lw` whole while this
            // closure lives, and nothing changes the kinds meanwhile
            let spill_kinds = lw.current_kinds.clone();
            let spill_slot = |bcx: &mut E, slot: usize| {
                let k = spill_kinds[off + slot];
                let Some(tag_byte) = known_tag(k) else {
                    return;
                };
                let slot_arg = bcx.ins().iconst(types::I64, slot as i64);
                let tag_arg = bcx.ins().iconst(types::I64, tag_byte as i64);
                let raw_arg = bcx.use_var(regs[slot]);
                bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
            };
            if !is_ipairs_trace {
                for slot in a_us..=(a_us + 2) {
                    spill_slot(&mut lw.bcx, slot);
                }
            }

            if is_ipairs_trace {
                emit_ipairs_tfor_call(lw, pl, oc, a_us, nvars, &spill_slot);
            } else {
                emit_tfor_helper_call(lw, pl, oc, a_us, nvars);
            }

            lw.current_kinds[off + a_us + 2] = RegKind::Unknown;
            lw.current_kinds[off + a_us + 4] = RegKind::Unknown;
            if (nvars as usize) >= 2 && a_us + 5 < max_stack {
                lw.current_kinds[off + a_us + 5] = RegKind::Unknown;
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// The helper-call path (used by slow_blk in the
/// ipairs case + the non-ipairs case wholesale).
/// Allocates the 3-slot buffer, calls the helper,
/// brif-checks the result, def_vars regs + tag from
/// the buffer.
pub(super) fn emit_tfor_helper_call<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    a_us: usize,
    nvars: i64,
) {
    let Plan { max_stack, .. } = *pl;
    let Lower {
        tforcall_tag_var,
        tforcall_val_tag_var,
        ..
    } = *lw;
    let OpHelpers { op_tforcall_id, .. } = lw.h.op;
    let OpCx { i, rop, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    let out_ss = lw
        .bcx
        .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            24,
            3,
        ));
    let ctrl_addr = lw.bcx.ins().stack_addr(types::I64, out_ss, 0);
    let key_addr = lw.bcx.ins().stack_addr(types::I64, out_ss, 8);
    let val_addr = lw.bcx.ins().stack_addr(types::I64, out_ss, 16);
    let a_arg = lw.bcx.ins().iconst(types::I64, a_us as i64);
    let nvars_arg = lw.bcx.ins().iconst(types::I64, nvars);
    let func_ref = lw.bcx.import_func(op_tforcall_id);
    let call_inst = lw
        .bcx
        .ins()
        .call(func_ref, &[a_arg, nvars_arg, ctrl_addr, key_addr, val_addr]);
    let status_or_tag = lw.bcx.inst_results(call_inst)[0];
    // -1: not a native iterator, or it raised; the
    // interpreter redoes the op
    let ok = lw
        .bcx
        .ins()
        .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, status_or_tag, 0);
    guard!(lw, pl, ok, i, rop.pc);
    // key tag | value tag << 8 (Vm::jit_op_tforcall)
    let key_tag = lw.bcx.ins().band_imm_u(status_or_tag, 0xff);
    let val_tag = lw.bcx.ins().ushr_imm_u(status_or_tag, 8);
    lw.bcx.def_var(tforcall_tag_var, key_tag);
    lw.bcx.def_var(tforcall_val_tag_var, val_tag);
    let ctrl_raw = lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 0);
    let key_raw = lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 8);
    let val_raw = lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 16);
    lw.bcx.def_var(regs[a_us + 2], ctrl_raw);
    lw.bcx.def_var(regs[a_us + 4], key_raw);
    if (nvars as usize) >= 2 && a_us + 5 < max_stack {
        lw.bcx.def_var(regs[a_us + 5], val_raw);
    }
}

/// `TForCall` over `ipairs`: the array read inline, the helper when it misses.
pub(super) fn emit_ipairs_tfor_call<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    a_us: usize,
    nvars: i64,
    spill_slot: &impl Fn(&mut E, usize),
) {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let Lower {
        tforcall_tag_var,
        tforcall_val_tag_var,
        ..
    } = *lw;
    let OpCx { i, rop, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    // Inline aget fast path. The recorder confirmed
    // R[A] = ipairs_iter at trace start. The standard
    // ipairs loop has R[A+1] = Table (state) and
    // R[A+2] = Int (control = last seen index).
    // Per iter: next_i = ctrl + 1; val = t[next_i].
    // If val is Nil → loop ends; else key = next_i,
    // val_raw = val's payload.
    let ctrl = lw.bcx.use_var(regs[a_us + 2]);
    let t_raw = lw.bcx.use_var(regs[a_us + 1]);
    let one = lw.bcx.ins().iconst(types::I64, 1);
    let next_i = lw.bcx.ins().iadd(ctrl, one);
    let key_m1 = ctrl;

    let asize = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        t_raw,
        crate::jit_backend::TABLE_ASIZE_OFFSET as i32,
    );
    let in_range = lw.bcx.ins().icmp(IntCC::UnsignedLessThan, key_m1, asize);
    let metatable = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        t_raw,
        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
    );
    let zero = lw.bcx.ins().iconst(types::I64, 0);
    let no_meta = lw.bcx.ins().icmp(IntCC::Equal, metatable, zero);
    let fast_ok = lw.bcx.ins().band(in_range, no_meta);

    let fast_blk = lw.bcx.create_block();
    let slow_blk = lw.bcx.create_block();
    let merge_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(fast_ok, fast_blk, &[], slow_blk, &[]);

    // ----- fast_blk: inline aget + populate -----
    lw.bcx.switch_to_block(fast_blk);
    lw.bcx.seal_block(fast_blk);
    let avals_ptr = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        t_raw,
        crate::jit_backend::TABLE_ARRAY_PTR_OFFSET as i32,
    );
    let three = lw.bcx.ins().iconst(types::I64, 3);
    let val_off = lw.bcx.ins().ishl(key_m1, three);
    let val_addr_fast = lw.bcx.ins().iadd(avals_ptr, val_off);
    let val_raw_fast = lw.bcx.ins().load(
        types::I64,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        val_addr_fast,
        0,
    );
    let avals_bytes = lw.bcx.ins().ishl(asize, three);
    let tag_base = lw.bcx.ins().iadd(avals_ptr, avals_bytes);
    let tag_addr = lw.bcx.ins().iadd(tag_base, key_m1);
    let val_tag_i8 = lw.bcx.ins().load(
        types::I8,
        cranelift_codegen::ir::MemFlagsData::trusted(),
        tag_addr,
        0,
    );
    let val_tag = lw.bcx.ins().uextend(types::I64, val_tag_i8);
    let nil_const = lw
        .bcx
        .ins()
        .iconst(types::I64, luna_core::runtime::value::raw::NIL as i64);
    let int_const = lw
        .bcx
        .ins()
        .iconst(types::I64, luna_core::runtime::value::raw::INT as i64);
    let is_nil = lw.bcx.ins().icmp(IntCC::Equal, val_tag, nil_const);
    // runtime val_tag guard. Snapshot
    // at recorder fire (R[A+5]'s tag) is the
    // *expected* iter val tag. The trace's
    // downstream emit (Move propagation, Concat
    // spill via RegKind::Str etc.) is specialised
    // to this tag. If a subsequent iter delivers a
    // different non-Nil tag (mixed-tag array), the
    // spill would pack stale bits as the snapshot
    // tag → garbage Value. Guard: `val_tag == Nil
    // OR val_tag == expected_tag` → continue, else
    // deopt. Skip the guard when no snapshot is
    // available (snapshot=None) or when the
    // snapshot is Nil itself (degenerate).
    if let Some(expected_tag) = record.tfor_val_tag
        && expected_tag != luna_core::runtime::value::raw::NIL
    {
        let exp_const = lw.bcx.ins().iconst(types::I64, expected_tag as i64);
        let is_exp = lw.bcx.ins().icmp(IntCC::Equal, val_tag, exp_const);
        let ok = lw.bcx.ins().bor(is_nil, is_exp);
        let guard_continue = lw.bcx.create_block();
        let guard_deopt = lw.bcx.create_block();
        lw.bcx.ins().brif(ok, guard_continue, &[], guard_deopt, &[]);
        lw.bcx.switch_to_block(guard_deopt);
        lw.bcx.seal_block(guard_deopt);
        // restored with the kinds the registers have here
        guard_exit(lw, pl, rop.pc, i);
        lw.bcx.switch_to_block(guard_continue);
        lw.bcx.seal_block(guard_continue);
    }
    let zero_raw = lw.bcx.ins().iconst(types::I64, 0);
    // R[A+4] = is_nil ? Nil(raw=0) : Int(raw=next_i)
    let r4_raw = lw.bcx.ins().select(is_nil, zero_raw, next_i);
    let r4_tag = lw.bcx.ins().select(is_nil, nil_const, int_const);
    lw.bcx.def_var(regs[a_us + 2], next_i);
    lw.bcx.def_var(regs[a_us + 4], r4_raw);
    if a_us + 5 < max_stack {
        // On the Nil branch, exit_tag[A+5] stays
        // `Untouched` (no per-side-exit override
        // for A+5), so the dispatcher restores
        // using entry_tag. If entry was a Str
        // slot, packing with raw=0 produces a null
        // Gc<LuaStr> → panic on the next interp
        // touch. Preserve the previous regs[A+5]
        // (= the last non-Nil iter's value) on the
        // Nil branch so the trace exit restore
        // sees a real GC pointer.
        let prev_v5 = lw.bcx.use_var(regs[a_us + 5]);
        let chosen_v5 = lw.bcx.ins().select(is_nil, prev_v5, val_raw_fast);
        lw.bcx.def_var(regs[a_us + 5], chosen_v5);
    }
    lw.bcx.def_var(tforcall_tag_var, r4_tag);
    lw.bcx.def_var(tforcall_val_tag_var, val_tag);
    lw.bcx.ins().jump(merge_blk, &[]);

    // ----- slow_blk: helper fallback -----
    lw.bcx.switch_to_block(slow_blk);
    lw.bcx.seal_block(slow_blk);
    // Spill R[A+2] (ctrl, the only slot that
    // changes per iter via TForLoop's writeback)
    // so the helper sees the trace's current
    // value. R[A]/R[A+1] still hold their entry
    // values in vm.stack.
    spill_slot(&mut lw.bcx, a_us + 2);
    emit_tfor_helper_call(lw, pl, oc, a_us, nvars);
    lw.bcx.ins().jump(merge_blk, &[]);

    // ----- merge_blk -----
    lw.bcx.switch_to_block(merge_blk);
    lw.bcx.seal_block(merge_blk);
}
