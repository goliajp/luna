use super::Node;
use crate::runtime::Table;

/// Byte offset of the hash part's node pointer within `Table`.
pub const TABLE_NODES_OFFSET: usize = std::mem::offset_of!(Table, nodes);

/// Byte offset of the `u32` node mask: node count - 1, or `u32::MAX`
/// when the hash part is empty.
pub const TABLE_NODE_MASK_OFFSET: usize = std::mem::offset_of!(Table, node_mask);

/// Byte offsets of the `u32` array-part counters `acount` and
/// `aprefix`, which the method JIT's inline array stores keep in step.
pub const TABLE_ACOUNT_OFFSET: usize = std::mem::offset_of!(Table, acount);
/// See [`TABLE_ACOUNT_OFFSET`].
pub const TABLE_APREFIX_OFFSET: usize = std::mem::offset_of!(Table, aprefix);

/// Byte offset of the byte of the table header's `aux` word that holds
/// the read-only bit ([`TABLE_READONLY_BYTE_MASK`]): a trace tests it
/// before it stores into a table inline.
pub const TABLE_READONLY_BYTE_OFFSET: usize = std::mem::offset_of!(Table, hdr)
    + crate::runtime::heap::AUX_OFFSET
    + if cfg!(target_endian = "little") { 3 } else { 0 };

/// The read-only bit within the byte at [`TABLE_READONLY_BYTE_OFFSET`].
pub const TABLE_READONLY_BYTE_MASK: u8 = (crate::runtime::heap::READONLY_AUX >> 24) as u8;

/// Byte offset of the key within `Node` (= 0): its tag here, its payload
/// 8 bytes on.
pub const NODE_KEY_OFFSET: usize = std::mem::offset_of!(Node, key_tag);

/// Byte offset of the `i32` index of the next node in a chain (`-1` at its
/// end) within `Node`.
pub const NODE_NEXT_OFFSET: usize = std::mem::offset_of!(Node, next);

/// Byte offset of `val: Value` within `Node` (= 16).
pub const NODE_VAL_OFFSET: usize = std::mem::offset_of!(Node, val);

/// Total `Node` size in bytes (= 32). Used as the stride
/// in `node_addr = nodes_ptr + slot_idx * SIZEOF_NODE`.
pub const SIZEOF_NODE: usize = std::mem::size_of::<Node>();

/// Static guard: pin the assumptions luna-jit relies on at compile
/// time. Layout drift here breaks IR emit, so trap it at compile
/// time rather than at trace-fire time.
const _: () = {
    assert!(NODE_KEY_OFFSET == 0);
    assert!(std::mem::offset_of!(Node, key_payload) == 8);
    assert!(NODE_VAL_OFFSET == 16);
    assert!(SIZEOF_NODE == 32);
};
