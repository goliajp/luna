//! Reading and writing a table's array part and taking its length inline,
//! the common case of `t[i]`, `t[i] = v` and `#t`; anything else goes
//! through the helpers.

use super::*;
use cranelift_codegen::ir::{Block, MemFlagsData};

const ACOUNT: i32 = super::super::TABLE_ACOUNT_OFFSET;
const APREFIX: i32 = super::super::TABLE_APREFIX_OFFSET;

/// `(avals, atags)`: the array part's values and, after them, its raw tags.
fn array_part<E: Emit>(bcx: &mut E, t: Value, asize: Value) -> (Value, Value) {
    let avals = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        super::super::TABLE_ARRAY_PTR_OFFSET as i32,
    );
    let bytes = bcx.ins().ishl_imm_u(asize, 3);
    let atags = bcx.ins().iadd(avals, bytes);
    (avals, atags)
}

fn load_asize<E: Emit>(bcx: &mut E, t: Value) -> Value {
    bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        super::super::TABLE_ASIZE_OFFSET as i32,
    )
}

/// Branch to `hit`, with the address of `t[key]`'s payload as its one
/// parameter, when the integer `key` falls in the array part and the slot
/// holds a value of raw tag `want` (not nil, so no `__index` is consulted);
/// to `miss` otherwise. Leaves the builder in no block; the caller seals
/// `hit` and `miss`.
pub(super) fn emit_array_get_check<E: Emit>(
    bcx: &mut E,
    t: Value,
    key: Value,
    want: u8,
    hit: Block,
    miss: Block,
) {
    let idx = bcx.ins().iadd_imm_s(key, -1);
    let asize = load_asize(bcx, t);
    // `key - 1 < asize` unsigned: also rejects keys below 1
    let in_bounds = bcx.ins().icmp(IntCC::UnsignedLessThan, idx, asize);
    let slot_blk = bcx.create_block();
    bcx.ins().brif(in_bounds, slot_blk, &[], miss, &[]);
    bcx.switch_to_block(slot_blk);
    bcx.seal_block(slot_blk);
    let (avals, atags) = array_part(bcx, t, asize);
    let tag_addr = bcx.ins().iadd(atags, idx);
    let tag = bcx
        .ins()
        .uload8(types::I64, MemFlagsData::trusted(), tag_addr, 0);
    let ok = bcx.ins().icmp_imm_u(IntCC::Equal, tag, i64::from(want));
    let off = bcx.ins().ishl_imm_u(idx, 3);
    let val_addr = bcx.ins().iadd(avals, off);
    bcx.ins().brif(ok, hit, &[val_addr.into()], miss, &[]);
}

/// `t[key] = val` for an integer `key` in the array part and a value of
/// raw tag `r` the collector does not trace (a collectable one needs the
/// write barrier), keeping the `acount` / `aprefix` counts in step as
/// `Table::note_atag_change` does: jump to `done` when stored, to `miss`
/// when the store is the helper's (out of the array part; a nil slot of a
/// table with a metatable, whose `__newindex` decides; a slot that extends
/// the non-nil prefix past further filled slots, which the table scans
/// for). Leaves the builder in no block; the caller seals `miss` and
/// `done`.
pub(super) fn emit_array_set<E: Emit>(
    bcx: &mut E,
    t: Value,
    key: Value,
    val: Value,
    r: u8,
    done: Block,
    miss: Block,
) {
    use luna_core::runtime::value::raw;
    debug_assert!(!raw::is_gc(r) && r != raw::NIL);
    let flags = MemFlagsData::trusted();
    let idx = bcx.ins().iadd_imm_s(key, -1);
    let asize = load_asize(bcx, t);
    let in_bounds = bcx.ins().icmp(IntCC::UnsignedLessThan, idx, asize);
    let slot_blk = bcx.create_block();
    bcx.ins().brif(in_bounds, slot_blk, &[], miss, &[]);

    bcx.switch_to_block(slot_blk);
    bcx.seal_block(slot_blk);
    let (avals, atags) = array_part(bcx, t, asize);
    let tag_addr = bcx.ins().iadd(atags, idx);
    let off = bcx.ins().ishl_imm_u(idx, 3);
    let val_addr = bcx.ins().iadd(avals, off);
    let old = bcx.ins().uload8(types::I64, flags, tag_addr, 0);
    let was_nil = bcx.ins().icmp_imm_u(IntCC::Equal, old, i64::from(raw::NIL));
    let store_blk = bcx.create_block();
    let fill_blk = bcx.create_block();
    bcx.ins().brif(was_nil, fill_blk, &[], store_blk, &[]);

    // a nil slot gains a value: only without a metatable
    bcx.switch_to_block(fill_blk);
    bcx.seal_block(fill_blk);
    let mt = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::TABLE_METATABLE_OFFSET as i32,
    );
    let no_mt = bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0);
    let count_blk = bcx.create_block();
    bcx.ins().brif(no_mt, count_blk, &[], miss, &[]);

    bcx.switch_to_block(count_blk);
    bcx.seal_block(count_blk);
    let acount = bcx.ins().load(types::I32, flags, t, ACOUNT);
    let acount = bcx.ins().iadd_imm_u(acount, 1);
    let aprefix = bcx.ins().load(types::I32, flags, t, APREFIX);
    let aprefix = bcx.ins().uextend(types::I64, aprefix);
    let at_prefix = bcx.ins().icmp(IntCC::Equal, idx, aprefix);
    let plain_blk = bcx.create_block();
    let prefix_blk = bcx.create_block();
    bcx.ins().brif(at_prefix, prefix_blk, &[], plain_blk, &[]);

    bcx.switch_to_block(plain_blk);
    bcx.seal_block(plain_blk);
    bcx.ins().store(flags, acount, t, ACOUNT);
    bcx.ins().jump(store_blk, &[]);

    // the prefix grows by this slot when the next one is nil or past the end
    bcx.switch_to_block(prefix_blk);
    bcx.seal_block(prefix_blk);
    let next = bcx.ins().iadd_imm_u(idx, 1);
    let at_end = bcx.ins().icmp(IntCC::Equal, next, asize);
    let grow_blk = bcx.create_block();
    let peek_blk = bcx.create_block();
    bcx.ins().brif(at_end, grow_blk, &[], peek_blk, &[]);

    bcx.switch_to_block(peek_blk);
    bcx.seal_block(peek_blk);
    let next_tag = bcx.ins().uload8(types::I64, flags, tag_addr, 1);
    let next_nil = bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, next_tag, i64::from(raw::NIL));
    bcx.ins().brif(next_nil, grow_blk, &[], miss, &[]);

    bcx.switch_to_block(grow_blk);
    bcx.seal_block(grow_blk);
    bcx.ins().store(flags, acount, t, ACOUNT);
    let next32 = bcx.ins().ireduce(types::I32, next);
    bcx.ins().store(flags, next32, t, APREFIX);
    bcx.ins().jump(store_blk, &[]);

    bcx.switch_to_block(store_blk);
    bcx.seal_block(store_blk);
    let tag = bcx.ins().iconst(types::I8, i64::from(r));
    bcx.ins().store(flags, tag, tag_addr, 0);
    bcx.ins().store(flags, val, val_addr, 0);
    bcx.ins().jump(done, &[]);
}

/// `#t` from the array part's counts: branch to `hit` with the length when
/// the table has no metatable and its non-nil slots are exactly a prefix
/// shorter than the array part (`Table::len`'s first case); to `miss`
/// otherwise.
pub(super) fn emit_len_check<E: Emit>(bcx: &mut E, t: Value, hit: Block, miss: Block) {
    let flags = MemFlagsData::trusted();
    let mt = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::TABLE_METATABLE_OFFSET as i32,
    );
    let no_mt = bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0);
    let acount = bcx.ins().load(types::I32, flags, t, ACOUNT);
    let aprefix = bcx.ins().load(types::I32, flags, t, APREFIX);
    let dense = bcx.ins().icmp(IntCC::Equal, acount, aprefix);
    let aprefix = bcx.ins().uextend(types::I64, aprefix);
    let asize = load_asize(bcx, t);
    let short = bcx.ins().icmp(IntCC::UnsignedLessThan, aprefix, asize);
    let ok = bcx.ins().band(no_mt, dense);
    let ok = bcx.ins().band(ok, short);
    bcx.ins().brif(ok, hit, &[aprefix.into()], miss, &[]);
}
