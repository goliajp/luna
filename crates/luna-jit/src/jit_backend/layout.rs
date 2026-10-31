//! Where compiled code finds the fields of luna's tables, strings and
//! nodes, and the raw tags it tests.

/// `Table` layout constants used by the inline-aset
/// fast path. Cranelift IR walks past the helper call ABI by
/// loading the table's array pointer and length directly from the
/// `Gc<Table>` raw ptr, skipping the per-iter thread-local read
/// + `Gc::from_ptr` non-null check + `Table::set_int` dispatch
/// that the helper-call path pays.
///
/// The offsets are computed at compile time via `std::mem::offset_of!`,
/// so the IR follows whatever `Table`'s `#[repr(C)]` layout chooses
/// today. The static asserts below pin the assumptions the IR
/// itself can't verify (`RawVal` packed to 8 bytes, and the `Table.asize` field width).
///
/// Table keeps `array_ptr: *mut u8` as the single
/// source of truth for "where does the array part live?". The pointer
/// targets either the inline storage embedded in the Table struct
/// (asize <= INLINE_ASIZE) or an external slab the table owns. The JIT
/// loads `array_ptr` directly — no branching, no indirection
/// — and computes `atags_ptr = array_ptr + asize * 8` on the fly.
pub(crate) const TABLE_ARRAY_PTR_OFFSET: usize =
    std::mem::offset_of!(luna_core::runtime::Table, array_ptr);
pub(crate) const TABLE_ASIZE_OFFSET: usize = std::mem::offset_of!(luna_core::runtime::Table, asize);
/// `Option<Gc<Table>>` is 8 bytes via NPO; 0 ⇔ None.
/// Inline aget reads this to short-circuit on metatable.is_none()
/// rather than always going through the helper's metatable check.
pub(crate) const TABLE_METATABLE_OFFSET: usize =
    std::mem::offset_of!(luna_core::runtime::Table, metatable);
pub(crate) const STR_SHORT_OFFSET: usize = luna_core::runtime::string::jit_layout::STR_SHORT_OFFSET;
/// A string's `u32` hash (see `field_slot::emit_str_key_absent`).
pub(crate) const STR_HASH_OFFSET: usize = luna_core::runtime::string::jit_layout::STR_HASH_OFFSET;
/// A string's `u32` byte length.
pub(crate) const STR_LEN_OFFSET: usize = luna_core::__private::STR_LEN_OFFSET;
/// A node's `i32` link to the next node of its chain (`-1` at the end).
pub(crate) const NODE_NEXT_OFFSET: usize = luna_core::runtime::table::jit_layout::NODE_NEXT_OFFSET;
pub(crate) const TABLE_ACOUNT_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_ACOUNT_OFFSET as i32;
pub(crate) const TABLE_APREFIX_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_APREFIX_OFFSET as i32;
/// A `NewTable`'s operands packed as `luna_jit_new_table_sized` takes them.
pub(crate) fn pack_table_ops(ins: luna_core::vm::isa::Inst) -> i64 {
    i64::from(ins.b()) | i64::from(ins.c()) << 8 | i64::from(ins.k()) << 16
}
pub(crate) const TABLE_ALIMIT_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_ALIMIT_OFFSET as i32;
pub(crate) const TABLE_LENHINT_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_LENHINT_OFFSET as i32;
pub(crate) const TABLE_DIALECT_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_DIALECT_OFFSET as i32;
/// The byte holding a table's read-only bit, and the bit: a trace's
/// inline stores leave a read-only table to the store helper, which
/// refuses it.
pub(crate) const TABLE_READONLY_BYTE_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_READONLY_BYTE_OFFSET as i32;
pub(crate) const TABLE_READONLY_BYTE_MASK: i64 =
    luna_core::runtime::table::jit_layout::TABLE_READONLY_BYTE_MASK as i64;

/// table-field IC scaffold.
///
/// Byte offset of the hash part's node pointer. luna-core's
/// `runtime::table::jit_layout` module computes this against the live
/// `Table` struct, then we re-export it here so trace.rs can refer to
/// it locally.
pub(crate) const TABLE_NODES_PTR_OFFSET: usize =
    luna_core::runtime::table::jit_layout::TABLE_NODES_OFFSET;
/// The `u32` node mask (node count - 1, `u32::MAX` when empty). The IC's
/// shape-stability guard compares it against the recorder's node count
/// less one.
pub(crate) const TABLE_NODE_MASK_OFFSET: usize =
    luna_core::runtime::table::jit_layout::TABLE_NODE_MASK_OFFSET;
/// Within one `Node`, the byte offset of `key: Value`. Value's tag
/// byte (`#[repr(C, u8)]`) lives at offset 0 of the Value, so the
/// key's tag is at `NODE_KEY_OFFSET` (= 0) and the key's raw
/// 8-byte payload at `NODE_KEY_OFFSET + 8`.
pub(crate) const NODE_KEY_OFFSET: usize = luna_core::runtime::table::jit_layout::NODE_KEY_OFFSET;
/// Byte offset of `val: Value` within a `Node`. The val's tag is
/// at `NODE_VAL_OFFSET` (= 16), payload at `NODE_VAL_OFFSET + 8`.
pub(crate) const NODE_VAL_OFFSET: usize = luna_core::runtime::table::jit_layout::NODE_VAL_OFFSET;
/// Total `Node` size in bytes — stride for `node_addr = nodes_ptr +
/// slot_idx * SIZEOF_NODE`.
pub(crate) const SIZEOF_NODE: usize = luna_core::runtime::table::jit_layout::SIZEOF_NODE;
/// Byte offset of the value's tag byte inside the `val: Value` field
/// of a `Node`. Value is `#[repr(C, u8)]`, discriminant at byte 0.
pub(crate) const NODE_VAL_TAG_OFFSET: usize = NODE_VAL_OFFSET;
/// Byte offset of the value's 8-byte raw payload inside `val: Value`.
/// 7 bytes of alignment padding sit between the tag and the payload.
pub(crate) const NODE_VAL_RAW_OFFSET: usize = NODE_VAL_OFFSET + 8;
/// Byte offset of the key's 8-byte raw payload inside `key: Value`.
/// IC's "slot key still matches" guard reads 8 bytes here and
/// compares against the recorder-cached `Gc<LuaStr>` pointer bits.
pub(crate) const NODE_KEY_RAW_OFFSET: usize = NODE_KEY_OFFSET + 8;

pub(super) const RAW_TAG_INT: i64 = luna_core::__private::raw::INT as i64;
pub(super) const RAW_TAG_FLOAT: i64 = luna_core::__private::raw::FLOAT as i64;
pub(super) const RAW_TAG_TABLE: i64 = luna_core::__private::raw::TABLE as i64;
pub(super) const RAW_TAG_NIL: i64 = luna_core::__private::raw::NIL as i64;

const _: () = {
    assert!(std::mem::size_of::<*mut u8>() == 8);
    assert!(std::mem::size_of::<luna_core::__private::RawVal>() == 8);
    assert!(std::mem::align_of::<luna_core::__private::RawVal>() == 8);
    // `asize` is a u32 the JIT loads zero-extended (`uload32`), then
    // shifts left 3 to multiply by 8 for the `atags_ptr = array_ptr +
    // asize * 8` computation.
    assert!(std::mem::size_of::<u32>() == 4);
};
