//! Reads of the trace's upvalues.

use super::*;

/// Upvalue `idx` read through the checked helper, typed `want`: the call
/// made once per trace, at the first read, and guarded there.
pub(super) fn checked_upval_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rop: &RecordedOp,
    idx: u32,
    want: u8,
) -> Option<Value> {
    let RuntimeHelpers {
        upval_get_checked_id,
        ..
    } = lw.h.rt;
    let bcx = &mut lw.bcx;
    let checked = lw.upval_checked.entry(idx).or_insert_with(|| {
        let var = bcx.declare_var(types::I64);
        let ss = bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let out = bcx.ins().stack_addr(types::I64, ss, 0);
        let idx_arg = bcx.ins().iconst(types::I64, i64::from(idx));
        let want_arg = bcx.ins().iconst(types::I64, i64::from(want));
        let f = bcx.import_func(upval_get_checked_id);
        let call = bcx.ins().call(f, &[idx_arg, want_arg, out]);
        let ok = bcx.inst_results(call)[0];
        (var, ss, ok, want)
    });
    let (var, ss, ok, checked_want) = *checked;
    if checked_want != want {
        return None;
    }
    // the first read of this upvalue made the check
    if !lw.upval_check_done.contains(&idx) {
        lw.upval_check_done.push(idx);
        guard!(lw, pl, ok, i, rop.pc);
        let v = lw.bcx.ins().stack_load(types::I64, types::I64, ss, 0);
        lw.bcx.def_var(var, v);
    }
    Some(lw.bcx.use_var(var))
}

/// The table in upvalue `idx` of op `i`'s function, checked to be a table
/// (the trace leaves at the op otherwise).
pub(in crate::jit_backend::trace::lower) fn upval_table_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rop: &RecordedOp,
    idx: u32,
) -> Option<Value> {
    let table = luna_core::runtime::value::raw::TABLE;
    if std::ptr::eq(rop.proto.as_ptr(), pl.head_proto.as_ptr()) {
        checked_upval_read(lw, pl, i, rop, idx, table)
    } else {
        Some(frame_upval_read(lw, pl, i, rop, idx, table))
    }
}

/// `GetUpval` in a function of another proto the trace inlined: read
/// through that frame's own closure, typed by the value the recording saw.
pub(super) fn emit_frame_upval_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let OpCx { i, off, ins, .. } = *oc;
    let Some(kind) = pl
        .record
        .result_tag(i)
        .and_then(RegKind::from_entry_tag)
        .filter(|k| !matches!(k, RegKind::Nil | RegKind::Bool))
    else {
        checkpoint("bail:inline-upval-untyped");
        return None;
    };
    let v = frame_upval_read(lw, pl, i, oc.rop, ins.b(), kind_tag(kind));
    lw.bcx.def_var(oc.regs[ins.a() as usize], v);
    lw.current_kinds[off + ins.a() as usize] = kind;
    Some(())
}

/// Upvalue `idx` of the closure running the inlined frame of op `oc` (the
/// value its caller called, one below the frame's window), checked to
/// have raw tag `want`; the trace leaves at the op otherwise.
pub(super) fn frame_upval_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rop: &RecordedOp,
    idx: u32,
    want: u8,
) -> Value {
    let RuntimeHelpers {
        upval_of_checked_id,
        ..
    } = lw.h.rt;
    let cl = lw.bcx.use_var(lw.regs_full[pl.frame_func[i] as usize]);
    let idx_arg = lw.bcx.ins().iconst(types::I64, i64::from(idx));
    let ss = lw
        .bcx
        .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
    let out = lw.bcx.ins().stack_addr(types::I64, ss, 0);
    let want_arg = lw.bcx.ins().iconst(types::I64, i64::from(want));
    let f = lw.bcx.import_func(upval_of_checked_id);
    let call = lw.bcx.ins().call(f, &[cl, idx_arg, want_arg, out]);
    let ok = lw.bcx.inst_results(call)[0];
    guard!(lw, pl, ok, i, rop.pc);
    lw.bcx.ins().stack_load(types::I64, types::I64, ss, 0)
}
