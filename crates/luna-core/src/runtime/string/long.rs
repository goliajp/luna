//! Long strings, which are not interned (but 5.1's).

use super::*;

/// A long string of `a` then `b` (together longer than `MAX_SHORT_LEN`).
pub(crate) fn alloc_long_pair(
    mem: MemRef,
    a: &[u8],
    b: &[u8],
    seed: u32,
    h51: bool,
) -> Result<*mut LuaStr, Oom> {
    debug_assert!(a.len() + b.len() > MAX_SHORT_LEN);
    if h51 {
        alloc_str_of(mem, &[a, b], false, lua_hash_51_pair(a, b), true)
    } else {
        alloc_str_of(mem, &[a, b], false, seed, false)
    }
}

/// A long string of `parts` one after another, hashed when first used as
/// a key (not 5.1's, whose long strings are interned).
pub(crate) fn alloc_long_parts(
    mem: MemRef,
    parts: &[&[u8]],
    seed: u32,
) -> Result<*mut LuaStr, Oom> {
    alloc_str_of(mem, parts, false, seed, false)
}

/// A long string: hashed when first used as a key, or at once with PUC
/// 5.1's hash (`h51`).
pub(crate) fn alloc_long(
    mem: MemRef,
    bytes: &[u8],
    seed: u32,
    h51: bool,
) -> Result<*mut LuaStr, Oom> {
    debug_assert!(bytes.len() > MAX_SHORT_LEN);
    if h51 {
        alloc_str(mem, bytes, false, lua_hash_51(bytes), true)
    } else {
        alloc_str(mem, bytes, false, seed, false)
    }
}
