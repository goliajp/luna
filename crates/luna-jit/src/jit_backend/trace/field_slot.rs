//! Reading and writing a table field in the hash slot the key was found
//! in while recording (LuaJIT's `HREFK`): one bounds check and a key
//! compare instead of a hash lookup through a helper.

use super::*;
use cranelift_codegen::ir::{Block, MemFlagsData};

/// The tag byte a `Value` of `raw` tag `r` carries in memory (the enum
/// discriminant, `value::tag`), for the kinds a trace reads or writes in
/// a slot.
fn mem_tag(r: u8) -> u8 {
    use luna_core::runtime::value::{raw, tag};
    match r {
        raw::INT => tag::INT,
        raw::FLOAT => tag::FLOAT,
        raw::STR => tag::STR,
        raw::TABLE => tag::TABLE,
        raw::CLOSURE => tag::CLOSURE,
        _ => unreachable!("no slot access for raw tag {r}"),
    }
}

/// Branch to `hit`, with the address of node `slot` of table `t` as its
/// one parameter, when that node exists, holds the interned string `key`
/// and a value of raw tag `want` (any non-nil value when `want` is
/// `None`); to `miss` otherwise. A non-nil value under the key means no
/// `__index` or `__newindex` would be consulted, so the table's metatable
/// does not matter; a store tests that the table is not read-only first.
/// Leaves the builder in no block; the caller seals `hit` and `miss`.
pub(super) fn emit_field_slot_check<E: Emit>(
    bcx: &mut E,
    t: Value,
    key: Value,
    slot: u32,
    want: Option<u8>,
    hit: Block,
    miss: Block,
) {
    use luna_core::runtime::value::tag;
    let flags = MemFlagsData::trusted();
    // the node count is the mask plus one, which wraps to 0 for the
    // `u32::MAX` of an empty hash part
    let mask = bcx.ins().load(
        types::I32,
        flags,
        t,
        super::super::TABLE_NODE_MASK_OFFSET as i32,
    );
    let in_bounds = if slot == 0 {
        bcx.ins()
            .icmp_imm_u(IntCC::NotEqual, mask, i64::from(u32::MAX))
    } else {
        let len = bcx.ins().iadd_imm_u(mask, 1);
        bcx.ins()
            .icmp_imm_u(IntCC::UnsignedGreaterThan, len, i64::from(slot))
    };
    let node_blk = bcx.create_block();
    bcx.ins().brif(in_bounds, node_blk, &[], miss, &[]);
    bcx.switch_to_block(node_blk);
    bcx.seal_block(node_blk);
    let nodes = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::TABLE_NODES_PTR_OFFSET as i32,
    );
    let node = bcx
        .ins()
        .iadd_imm_u(nodes, (slot as usize * super::super::SIZEOF_NODE) as i64);
    let key_tag = bcx.ins().uload8(
        types::I64,
        flags,
        node,
        super::super::NODE_KEY_OFFSET as i32,
    );
    let key_raw = bcx.ins().load(
        types::I64,
        flags,
        node,
        super::super::NODE_KEY_RAW_OFFSET as i32,
    );
    let val_tag = bcx.ins().uload8(
        types::I64,
        flags,
        node,
        super::super::NODE_VAL_TAG_OFFSET as i32,
    );
    // one compare and branch each: combined with `band` the three
    // compares could not be fused into their branch without an optimizer
    let key_ok = bcx.ins().icmp(IntCC::Equal, key_raw, key);
    branch_or_miss(bcx, key_ok, miss);
    let tag_ok = bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, key_tag, i64::from(tag::STR));
    branch_or_miss(bcx, tag_ok, miss);
    let val_ok = match want {
        Some(w) => bcx
            .ins()
            .icmp_imm_u(IntCC::Equal, val_tag, i64::from(mem_tag(w))),
        None => bcx
            .ins()
            .icmp_imm_u(IntCC::NotEqual, val_tag, i64::from(tag::NIL)),
    };
    bcx.ins().brif(val_ok, hit, &[node.into()], miss, &[]);
}

/// Goes on in a new block when `ok`, else to `miss`.
fn branch_or_miss<E: Emit>(bcx: &mut E, ok: Value, miss: Block) {
    let next = bcx.create_block();
    bcx.ins().brif(ok, next, &[], miss, &[]);
    bcx.switch_to_block(next);
    bcx.seal_block(next);
}

/// Load the value payload of the node at `node` (from
/// [`emit_field_slot_check`]'s `hit`).
pub(super) fn emit_slot_load<E: Emit>(bcx: &mut E, node: Value) -> Value {
    bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        node,
        super::super::NODE_VAL_RAW_OFFSET as i32,
    )
}

/// Overwrite the value of the node at `node` with `val` of raw tag `r`.
/// Only for a value the collector does not trace: a store of a
/// collectable one into a table needs the write barrier.
pub(super) fn emit_slot_store<E: Emit>(bcx: &mut E, node: Value, val: Value, r: u8) {
    debug_assert!(!luna_core::runtime::value::raw::is_gc(r));
    let flags = MemFlagsData::trusted();
    let tag = bcx.ins().iconst(types::I8, i64::from(mem_tag(r)));
    bcx.ins()
        .store(flags, tag, node, super::super::NODE_VAL_TAG_OFFSET as i32);
    bcx.ins()
        .store(flags, val, node, super::super::NODE_VAL_RAW_OFFSET as i32);
}

/// Branch to `absent` when table `t` has no node holding the interned
/// string `key` (found by walking at most `hops` nodes of its chain, as
/// `Table::get_str` does), to `unsure` otherwise (the key is there, or the
/// chain is longer). Leaves the builder in no block; the caller seals both.
pub(super) fn emit_str_key_absent<E: Emit>(
    bcx: &mut E,
    t: Value,
    key: Value,
    hops: usize,
    absent: Block,
    unsure: Block,
) {
    use luna_core::runtime::value::tag;
    let flags = MemFlagsData::trusted();
    let mask = bcx.ins().load(
        types::I32,
        flags,
        t,
        super::super::TABLE_NODE_MASK_OFFSET as i32,
    );
    let mask = bcx.ins().uextend(types::I64, mask);
    // an empty hash part has the mask `u32::MAX`
    let empty = bcx.ins().ushr_imm_u(mask, 31);
    let probe = bcx.create_block();
    bcx.ins().brif(empty, absent, &[], probe, &[]);
    bcx.switch_to_block(probe);
    bcx.seal_block(probe);
    let nodes = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::TABLE_NODES_PTR_OFFSET as i32,
    );
    let hash = bcx
        .ins()
        .load(types::I32, flags, key, super::super::STR_HASH_OFFSET as i32);
    let hash = bcx.ins().uextend(types::I64, hash);
    let mut idx = bcx.ins().band(hash, mask);
    for _ in 0..hops {
        let off = bcx
            .ins()
            .ishl_imm_u(idx, i64::from(super::super::SIZEOF_NODE.trailing_zeros()));
        let node = bcx.ins().iadd(nodes, off);
        let key_tag = bcx.ins().uload8(
            types::I64,
            flags,
            node,
            super::super::NODE_KEY_OFFSET as i32,
        );
        let key_raw = bcx.ins().load(
            types::I64,
            flags,
            node,
            super::super::NODE_KEY_RAW_OFFSET as i32,
        );
        let tag_ok = bcx
            .ins()
            .icmp_imm_u(IntCC::Equal, key_tag, i64::from(tag::STR));
        let key_ok = bcx.ins().icmp(IntCC::Equal, key_raw, key);
        let here = bcx.ins().band(tag_ok, key_ok);
        let next_blk = bcx.create_block();
        bcx.ins().brif(here, unsure, &[], next_blk, &[]);
        bcx.switch_to_block(next_blk);
        bcx.seal_block(next_blk);
        let next = bcx.ins().load(
            types::I32,
            flags,
            node,
            super::super::NODE_NEXT_OFFSET as i32,
        );
        let next = bcx.ins().uextend(types::I64, next);
        // the chain ends at -1
        let end = bcx.ins().ushr_imm_u(next, 31);
        let follow = bcx.create_block();
        bcx.ins().brif(end, absent, &[], follow, &[]);
        bcx.switch_to_block(follow);
        bcx.seal_block(follow);
        idx = next;
    }
    bcx.ins().jump(unsure, &[]);
}
