//! Compact (tag, payload) encoding used by table array parts — the 5.5
//! "compact arrays" layout: 1 tag byte + 8 payload bytes per slot. The
//! payload is a union (PUC `Value` union shape) rather than u64 bits so that
//! pointer provenance survives the round-trip (strict-provenance clean).

use super::*;

/// The tag of `nil`.
pub const NIL: u8 = 0;
/// The tag of `false`.
pub const FALSE: u8 = 1;
/// The tag of `true`.
pub const TRUE: u8 = 2;
/// The tag of an integer.
pub const INT: u8 = 3;
/// The tag of a float.
pub const FLOAT: u8 = 4;
/// The tag of a string.
pub const STR: u8 = 5;
/// The tag of a table.
pub const TABLE: u8 = 6;
/// The tag of a Lua function.
pub const CLOSURE: u8 = 7;
/// The tag of a native function.
pub const NATIVE: u8 = 8;
/// The tag of a coroutine.
pub const CORO: u8 = 9;
/// The tag of a full userdata.
pub const USERDATA: u8 = 10;
/// The tag of a light userdata.
pub const LIGHTUSERDATA: u8 = 11;

/// Heap-managed tags.
pub fn is_gc(tag: u8) -> bool {
    // LIGHTUSERDATA is an opaque host pointer (PUC void*); not GC-managed.
    (STR..LIGHTUSERDATA).contains(&tag)
}

/// A value's payload in the array-part encoding, read by its tag.
#[derive(Clone, Copy)]
pub union RawVal {
    /// all bits, zero for the variants without a payload
    pub zero: u64,
    /// an integer
    pub i: i64,
    /// a float
    pub f: f64,
    /// a string
    pub s: *mut LuaStr,
    /// a table
    pub t: *mut Table,
    /// a Lua function
    pub c: *mut LuaClosure,
    /// a native function
    pub n: *mut NativeClosure,
    /// a coroutine
    pub co: *mut Coro,
    /// a full userdata
    pub u: *mut Userdata,
    /// a light userdata
    pub lu: *const (),
}

impl RawVal {
    pub(crate) const NIL: RawVal = RawVal { zero: 0 };
}
