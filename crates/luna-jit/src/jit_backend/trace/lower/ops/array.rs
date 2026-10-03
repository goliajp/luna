use super::*;
use cranelift_codegen::ir::MemFlagsData;

/// `t[key]` for an integer key, typed `want`: from the array part when
/// the slot is there and holds that type; otherwise the trace leaves for
/// the interpreter, which runs the read. A helper call in its place
/// doubled the IR a read takes, and Cranelift's time with it.
pub(super) fn array_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    t: Value,
    key: Value,
    want: u8,
) -> Value {
    let OpCx { i, rop, .. } = *oc;
    let hit = lw.bcx.create_block();
    lw.bcx.append_block_param(hit, types::I64);
    let miss = lw.bcx.create_block();
    let merge = lw.bcx.create_block();
    lw.bcx.append_block_param(merge, types::I64);
    array_slot::emit_array_get_check(&mut lw.bcx, t, key, want, hit, miss);
    lw.bcx.switch_to_block(hit);
    lw.bcx.seal_block(hit);
    let addr = lw.bcx.block_params(hit)[0];
    let fast = lw
        .bcx
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 0);
    lw.bcx.ins().jump(merge, &[fast.into()]);
    lw.bcx.switch_to_block(miss);
    lw.bcx.seal_block(miss);
    guard_exit(lw, pl, rop.pc, i);
    lw.bcx.switch_to_block(merge);
    lw.bcx.seal_block(merge);
    lw.bcx.block_params(merge)[0]
}

/// `t[key] = val` into the array part for a number value: emits the
/// inline store and leaves the builder in its miss block, where the
/// caller emits the helper store and then calls [`array_write_join`] with
/// the returned block.
pub(super) fn array_write<E: Emit>(
    bcx: &mut E,
    t: Value,
    key: Value,
    val: Value,
    kind: RegKind,
) -> Option<Block> {
    if !matches!(kind, RegKind::Int | RegKind::Float) {
        return None;
    }
    let done = bcx.create_block();
    let miss = bcx.create_block();
    array_slot::emit_array_set(bcx, t, key, val, kind_tag(kind), done, miss);
    bcx.switch_to_block(miss);
    bcx.seal_block(miss);
    Some(done)
}

/// Joins the helper store with the inline one [`array_write`] emitted.
pub(super) fn array_write_join<E: Emit>(bcx: &mut E, stored_inline: Option<Block>) {
    if let Some(stored_inline) = stored_inline {
        bcx.ins().jump(stored_inline, &[]);
        bcx.switch_to_block(stored_inline);
        bcx.seal_block(stored_inline);
    }
}

/// A float key `key` (its bits) as an array index: the integer it equals,
/// or 0, which no array part holds, when it has a fraction, is out of the
/// integer range, is NaN or is -0.0 (a key of its own in 5.1/5.2). An
/// access by 0 misses the inline path and goes the way a miss goes, with
/// the float key itself.
pub(super) fn float_key_index<E: Emit>(bcx: &mut E, key: Value) -> Value {
    let (i, exact) = float_exact_int(bcx, key);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().select(exact, i, zero)
}
