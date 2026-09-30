use super::Node;
use crate::runtime::Table;

/// Byte offset of the `nodes: Box<[Node]>` field within `Table`.
/// The fat-ptr low word (data ptr) lives at this offset; the
/// high word (length) at `TABLE_NODES_OFFSET + 8`. luna-jit
/// adds `TABLE_NODES_PTR_OFFSET` / `TABLE_NODES_LEN_OFFSET`
/// constants in `jit_backend/mod.rs` to express that split.
pub const TABLE_NODES_OFFSET: usize = std::mem::offset_of!(Table, nodes);

/// Byte offsets of the `u32` array-part counters `acount` and
/// `aprefix`, which the method JIT's inline array stores keep in step.
pub const TABLE_ACOUNT_OFFSET: usize = std::mem::offset_of!(Table, acount);
/// See [`TABLE_ACOUNT_OFFSET`].
pub const TABLE_APREFIX_OFFSET: usize = std::mem::offset_of!(Table, aprefix);

/// Byte offset of `key: Value` within `Node` (= 0).
pub const NODE_KEY_OFFSET: usize = std::mem::offset_of!(Node, key);

/// Byte offset of `val: Value` within `Node` (= 16 — `key` is 16-byte
/// `Value`, no inner padding).
pub const NODE_VAL_OFFSET: usize = std::mem::offset_of!(Node, val);

/// Total `Node` size in bytes (= 40 on 64-bit). Used as the stride
/// in `node_addr = nodes_ptr + slot_idx * SIZEOF_NODE`.
pub const SIZEOF_NODE: usize = std::mem::size_of::<Node>();

/// Static guard: pin the assumptions luna-jit relies on at compile
/// time. Layout drift here breaks IR emit, so trap it at compile
/// time rather than at trace-fire time.
///
/// `Box<[T]>` is a fat pointer of `2 * usize` — 16 bytes on 64-bit
/// targets, 8 bytes on 32-bit (e.g. `wasm32`). Use a width-aware
/// expected size so the wasm32-unknown-unknown CI build does not
/// trip the assertion. The runtime layout still matters for luna-jit
/// IR emit on 64-bit hosts (the only platforms where Cranelift JIT
/// runs); the 32-bit branch documents the size in passing.
const _: () = {
    assert!(std::mem::size_of::<Box<[Node]>>() == 2 * std::mem::size_of::<usize>());
    assert!(NODE_KEY_OFFSET == 0);
    assert!(NODE_VAL_OFFSET == 16);
    assert!(SIZEOF_NODE >= 32);
};
