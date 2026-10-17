//! What each block of memory is for, as a host allocation function sees it.

use crate::version::LuaVersion;

/// What a block of memory is for. A new block's kind reaches a host
/// allocation function as its `osize`, in the dialect's numbering
/// (PUC's object type tags; 0 for anything that is not an object).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BlockKind {
    /// not an object: arrays, buffers, records
    Other,
    /// a string
    Str,
    /// a table
    Table,
    /// a Lua function (closure)
    LuaFn,
    /// a native function
    NativeFn,
    /// a full userdata
    Userdata,
    /// a thread (coroutine)
    Thread,
    /// a function prototype
    Proto,
    /// an upvalue
    Upvalue,
}

pub(super) const KINDS: usize = 9;

/// The `osize` each [`BlockKind`] has for a new block in dialect `v`.
pub(super) fn kind_codes(v: LuaVersion) -> [u8; KINDS] {
    match v {
        LuaVersion::Lua51 => [0; KINDS],
        LuaVersion::Lua52 => [0, 4, 5, 6, 6, 7, 8, 9, 10],
        // 5.3 upvalues are reference counted, not objects
        LuaVersion::Lua53 => [0, 4, 5, 6, 6, 7, 8, 9, 0],
        _ => [0, 4, 5, 6, 6, 7, 8, 10, 9],
    }
}
