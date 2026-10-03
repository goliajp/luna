use super::*;
use cranelift_codegen::ir::MemFlagsData;

/// `SelfOp`: `R[A+1] := R[B]`, then the method `R[B][K[C]]`, looked up in
/// the receiver and its table-valued `__index` links and checked against
/// the type the recording saw.
pub(super) fn emit_self_op<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    let RuntimeHelpers {
        op_self_checked_id, ..
    } = lw.h.rt;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    let (a, b) = (ins.a() as usize, ins.b() as usize);
    if k_op(&lw.current_kinds, (off + b) as u32) != RegKind::Table {
        checkpoint("bail:self-op-receiver-not-table");
        return None;
    }
    let Some(kind) = pl
        .record
        .result_tag(i)
        .and_then(RegKind::from_entry_tag)
        .filter(|k| !matches!(k, RegKind::Nil | RegKind::Bool))
    else {
        checkpoint("bail:self-op-result-untyped");
        return None;
    };
    let want = kind_tag(kind);
    let t = lw.bcx.use_var(regs[b]);
    let key_v = match rop.proto.consts[ins.c() as usize] {
        luna_core::runtime::Value::Str(s) => s,
        _ => unreachable!("pre-emit gates Str const at K[C]"),
    };
    let key_arg = emit_str_key_arg(&mut lw.bcx, key_v, pl.opts.aot, &mut lw.defined_aot_data);
    let v = match (pl.record.index_slots(i), pl.record.index_key) {
        (Some(slots), Some(index_key)) if matches!(kind, RegKind::Closure | RegKind::Table) => {
            emit_self_inline(lw, pl, oc, t, key_arg, index_key, slots, want)
        }
        _ => checked_read!(lw, pl, op_self_checked_id, t, key_arg, want, rop.pc, i),
    };
    lw.bcx.def_var(regs[a + 1], t);
    lw.current_kinds[off + a + 1] = RegKind::Table;
    lw.bcx.def_var(regs[a], v);
    lw.current_kinds[off + a] = kind;
    Some(())
}

/// The method lookup of a `SelfOp` where the recording found it: the
/// receiver `t` lacks `key`, its metatable's `__index` (hash slot
/// `slots.0`) is a table and holds `key` in slot `slots.1` with a value of
/// raw tag `want`. Anything else takes the checked helper.
#[allow(clippy::too_many_arguments)]
fn emit_self_inline<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    t: Value,
    key_arg: Value,
    index_key: luna_core::runtime::Gc<luna_core::runtime::LuaStr>,
    slots: (u32, u32),
    want: u8,
) -> Value {
    let RuntimeHelpers {
        op_self_checked_id, ..
    } = lw.h.rt;
    let OpCx { i, rop, .. } = *oc;
    let index_arg = emit_str_key_arg(
        &mut lw.bcx,
        index_key,
        pl.opts.aot,
        &mut lw.defined_aot_data,
    );
    let bcx = &mut lw.bcx;
    let absent = bcx.create_block();
    let slow = bcx.create_block();
    let merge = bcx.create_block();
    bcx.append_block_param(merge, types::I64);
    field_slot::emit_str_key_absent(bcx, t, key_arg, 2, absent, slow);
    bcx.switch_to_block(absent);
    bcx.seal_block(absent);
    let mt = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        crate::jit_backend::TABLE_METATABLE_OFFSET as i32,
    );
    let has_mt = bcx.ins().icmp_imm_u(IntCC::NotEqual, mt, 0);
    let mt_blk = bcx.create_block();
    bcx.ins().brif(has_mt, mt_blk, &[], slow, &[]);
    bcx.switch_to_block(mt_blk);
    bcx.seal_block(mt_blk);
    let link_hit = bcx.create_block();
    bcx.append_block_param(link_hit, types::I64);
    let table = luna_core::runtime::value::raw::TABLE;
    field_slot::emit_field_slot_check(bcx, mt, index_arg, slots.0, Some(table), link_hit, slow);
    bcx.switch_to_block(link_hit);
    bcx.seal_block(link_hit);
    let link_node = bcx.block_params(link_hit)[0];
    let link = field_slot::emit_slot_load(bcx, link_node);
    let hit = bcx.create_block();
    bcx.append_block_param(hit, types::I64);
    field_slot::emit_field_slot_check(bcx, link, key_arg, slots.1, Some(want), hit, slow);
    bcx.switch_to_block(hit);
    bcx.seal_block(hit);
    let node = bcx.block_params(hit)[0];
    let fast = field_slot::emit_slot_load(bcx, node);
    bcx.ins().jump(merge, &[fast.into()]);
    bcx.switch_to_block(slow);
    bcx.seal_block(slow);
    let v = checked_read!(lw, pl, op_self_checked_id, t, key_arg, want, rop.pc, i);
    lw.bcx.ins().jump(merge, &[v.into()]);
    lw.bcx.switch_to_block(merge);
    lw.bcx.seal_block(merge);
    lw.bcx.block_params(merge)[0]
}
