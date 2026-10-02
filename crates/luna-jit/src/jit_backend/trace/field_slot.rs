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
        _ => unreachable!("no slot access for raw tag {r}"),
    }
}

/// Branch to `hit`, with the address of node `slot` of table `t` as its
/// one parameter, when that node exists, holds the interned string `key`
/// and a value of raw tag `want` (any non-nil value when `want` is
/// `None`); to `miss` otherwise. A non-nil value under the key means no
/// `__index` or `__newindex` would be consulted, so the table's metatable
/// does not matter. Leaves the builder in no block; the caller seals `hit`
/// and `miss`.
pub(super) fn emit_field_slot_check(
    bcx: &mut FunctionBuilder<'_>,
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
    let len = bcx.ins().iadd_imm_u(mask, 1);
    let in_bounds = bcx
        .ins()
        .icmp_imm_u(IntCC::UnsignedGreaterThan, len, i64::from(slot));
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
    let tag_ok = bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, key_tag, i64::from(tag::STR));
    let key_ok = bcx.ins().icmp(IntCC::Equal, key_raw, key);
    let val_ok = match want {
        Some(w) => bcx
            .ins()
            .icmp_imm_u(IntCC::Equal, val_tag, i64::from(mem_tag(w))),
        None => bcx
            .ins()
            .icmp_imm_u(IntCC::NotEqual, val_tag, i64::from(tag::NIL)),
    };
    let ok = bcx.ins().band(tag_ok, key_ok);
    let ok = bcx.ins().band(ok, val_ok);
    bcx.ins().brif(ok, hit, &[node.into()], miss, &[]);
}

/// Load the value payload of the node at `node` (from
/// [`emit_field_slot_check`]'s `hit`).
pub(super) fn emit_slot_load(bcx: &mut FunctionBuilder<'_>, node: Value) -> Value {
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
pub(super) fn emit_slot_store(bcx: &mut FunctionBuilder<'_>, node: Value, val: Value, r: u8) {
    debug_assert!(!luna_core::runtime::value::raw::is_gc(r));
    let flags = MemFlagsData::trusted();
    let tag = bcx.ins().iconst(types::I8, i64::from(mem_tag(r)));
    bcx.ins()
        .store(flags, tag, node, super::super::NODE_VAL_TAG_OFFSET as i32);
    bcx.ins()
        .store(flags, val, node, super::super::NODE_VAL_RAW_OFFSET as i32);
}
