//! `TForCall` over `ipairs`.

use super::*;

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
    let lay = oc.ins.op().for_layout().expect("a loop op");
    let (ctl, first) = (a_us + lay.control() as usize, a_us + lay.var() as usize);
    // Inline aget fast path. The recorder confirmed
    // R[A] = ipairs_iter at trace start. The standard
    // ipairs loop has R[A+1] = Table (state) and
    // R[A+2] = Int (control = last seen index).
    // Per iter: next_i = ctrl + 1; val = t[next_i].
    // If val is Nil → loop ends; else key = next_i,
    // val_raw = val's payload.
    let ctrl = lw.bcx.use_var(regs[ctl]);
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
    // a table that may be 5.4's is bounded by `alimit`: an index past it
    // goes to the helper, which raises it (see `Table::array_index`)
    use crate::jit_backend::trace::array_slot::TableRules;
    let bound = if matches!(
        TableRules::of(pl.opts.dialect),
        TableRules::V54 | TableRules::Any
    ) {
        let len_flags = lw.bcx.len_state_flags();
        let alimit = lw.bcx.ins().load(
            types::I32,
            len_flags,
            t_raw,
            crate::jit_backend::TABLE_ALIMIT_OFFSET,
        );
        lw.bcx.ins().uextend(types::I64, alimit)
    } else {
        asize
    };
    let in_range = lw.bcx.ins().icmp(IntCC::UnsignedLessThan, key_m1, bound);
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
    if lay.copies_control() {
        lw.bcx.def_var(regs[ctl], next_i);
    }
    lw.bcx.def_var(regs[first], r4_raw);
    if first + 1 < max_stack {
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
        let prev_v5 = lw.bcx.use_var(regs[first + 1]);
        let chosen_v5 = lw.bcx.ins().select(is_nil, prev_v5, val_raw_fast);
        lw.bcx.def_var(regs[first + 1], chosen_v5);
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
    spill_slot(&mut lw.bcx, ctl);
    emit_tfor_helper_call(lw, pl, oc, a_us, nvars);
    lw.bcx.ins().jump(merge_blk, &[]);

    // ----- merge_blk -----
    lw.bcx.switch_to_block(merge_blk);
    lw.bcx.seal_block(merge_blk);
}
