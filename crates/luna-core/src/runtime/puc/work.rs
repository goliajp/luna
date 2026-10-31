//! PUC's sizes of an object and its parts, read from luna's object, by
//! dialect (`objsize`, what freeing it takes off the count).

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::size::PucSizes;

/// PUC's size of the object behind `h` other than a thread, in dialect `d`;
/// 0 for an object of luna's own.
///
/// # Safety
/// `h` heads an allocated object other than a thread.
pub(in crate::runtime::puc) unsafe fn objsize(s: &PucSizes, d: Dialect, h: *mut GcHeader) -> usize {
    // SAFETY: the caller's contract; the tag names the type
    unsafe {
        if (*h).is_uncounted() {
            return 0;
        }
        match (*h).tag {
            ObjTag::Table => {
                let t = h as *const Table;
                if (*h).cls & UNLISTED != 0 {
                    return 0;
                }
                s.table
                    + array_bytes(s, d, (*t).asize as usize)
                    + hash_bytes(s, d, (*t).node_count())
            }
            ObjTag::Proto => {
                s.proto + (*(h as *const Proto)).puc_arrays.get().iter().sum::<u32>() as usize
            }
            ObjTag::Closure => {
                let env = (*h).sub & crate::runtime::puc::ENV51 != 0;
                let n = (*(h as *const LuaClosure)).upvals_len as usize - usize::from(env);
                s.lua_closure(n)
            }
            ObjTag::Upvalue => s.upval,
            ObjTag::Native => {
                let n = native_upvals(h);
                if n > 0 || d == Dialect::L51 {
                    s.c_closure(n)
                } else {
                    0
                }
            }
            ObjTag::Userdata => (*(h as *const Userdata)).puc_size,
            ObjTag::Str => {
                let st = &*(h as *const LuaStr);
                match (*h).sub {
                    f if f & EXTERNAL != 0 => s.lngstr_hdr + 2 * s.ptr,
                    f if f & crate::runtime::puc::FIXED_EXT != 0 => s.lngstr_hdr,
                    _ => s.string(st.len(), !st.is_short() && d == Dialect::L55),
                }
            }
            ObjTag::Ghost if crate::runtime::puc::records::parse::is_load_tab(h) => {
                crate::runtime::puc::records::parse::load_tab_size(s, d, h)
            }
            ObjTag::Coro | ObjTag::Ghost => 0,
        }
    }
}

/// How many of `n`'s upvalues PUC's function has (the first ones).
#[inline(always)]
pub(in crate::runtime) fn native_puc_upvals(n: &NativeClosure) -> usize {
    match n.hdr.sub & !UNCOUNTED {
        0 => n.upvals.len(),
        k => k as usize - 1,
    }
}

/// The upvalues of PUC's C function standing for the native behind `h`.
///
/// # Safety
/// `h` heads an allocated native function.
pub(in crate::runtime::puc) unsafe fn native_upvals(h: *mut GcHeader) -> usize {
    // SAFETY: the caller's contract
    native_puc_upvals(unsafe { &*(h as *const NativeClosure) })
}

/// The bytes of a table's array part of `n` slots.
pub(in crate::runtime::puc) fn array_bytes(s: &PucSizes, d: Dialect, n: usize) -> usize {
    match (d, n) {
        (_, 0) => 0,
        // 5.5 keeps the values, then a byte per tag, around an unsigned
        (Dialect::L55, n) => n * 9 + 4,
        (_, n) => n * s.tvalue,
    }
}

/// The bytes of a table's hash part of `h` nodes: 5.5 puts a `lastfree`
/// pointer (`Limbox`, padded to a node's alignment) in front of eight
/// nodes or more.
pub(in crate::runtime::puc) fn hash_bytes(s: &PucSizes, d: Dialect, h: usize) -> usize {
    let limbox = if cfg!(target_arch = "wasm32") {
        8
    } else {
        s.ptr
    };
    match d {
        Dialect::L55 if h >= 8 => h * s.node + limbox,
        _ => h * s.node,
    }
}

/// PUC's size of a thread with a stack of `stack` slots and `ci`
/// `CallInfo`s (5.1: the size of its array).
pub(in crate::runtime::puc) fn thread_bytes(
    s: &PucSizes,
    d: Dialect,
    stack: u32,
    ci: u32,
) -> usize {
    s.thread + stack_bytes(s, d, stack) + s.callinfo * ci as usize
}

/// The bytes of a stack of `n` slots (PUC's `stacksize`, which from 5.4
/// leaves out the `EXTRA_STACK` slots it allocates beyond it).
pub(in crate::runtime::puc) fn stack_bytes(s: &PucSizes, d: Dialect, n: u32) -> usize {
    match (n, d >= Dialect::L54) {
        (0, _) => 0,
        (n, true) => (n as usize + 5) * s.tvalue,
        (n, false) => n as usize * s.tvalue,
    }
}
