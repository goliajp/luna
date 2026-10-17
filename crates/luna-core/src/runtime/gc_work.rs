//! What PUC's collector counts for an object, read from luna's object: its
//! size (`objsize`, what freeing it takes off the count) and the work of
//! traversing it (`propagatemark`'s result), by dialect.

use super::*;
use crate::runtime::puc_size::PucSizes;
use crate::runtime::table::Dialect;

impl Heap {
    /// PUC's size of the object behind `h`, as the count has it; 0 for an
    /// object of luna's own.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(crate) unsafe fn puc_objsize(&self, h: *mut GcHeader) -> usize {
        // SAFETY: the caller's contract
        unsafe {
            if (*h).tag == ObjTag::Coro {
                if is_uncounted(h) {
                    return 0;
                }
                let c = &*(h as *const crate::runtime::Coro);
                let ci = if self.table_dialect == Dialect::L51 {
                    c.frame_size
                } else {
                    c.ci_alloc
                };
                return self.thread_bytes(c.stack_size, ci);
            }
            objsize(&self.sizes, self.table_dialect, &self.puc.own_tables, h)
        }
    }

    /// The work PUC's `propagatemark` counts for traversing the object
    /// behind `h` (5.1 to 5.3 in bytes, before the strings and userdata it
    /// marks, which the marker adds; 5.4 and 5.5 in slots). `run` holds the
    /// top, the stack's size and the `CallInfo`s of the running thread,
    /// which the VM holds.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(crate) unsafe fn object_work(&self, h: *mut GcHeader, run: Running) -> i64 {
        let run = (h == run.h).then_some((run.top, run.size, run.ci));
        // SAFETY: the caller's contract
        unsafe {
            let d = self.table_dialect;
            // a function of PUC's C closures, the library's too (made
            // without counting), at PUC's number of upvalues; from 5.2 on
            // one with none is a light function, no object
            if (*h).tag == ObjTag::Native {
                let n = native_upvals(h);
                return match d {
                    Dialect::L51 => self.sizes.c_closure(n) as i64,
                    _ if n == 0 => 0,
                    Dialect::L52 | Dialect::L53 => self.sizes.c_closure(n) as i64,
                    _ => 1 + n as i64,
                };
            }
            // the main thread is luna's own, with a stack PUC has
            if is_uncounted(h) && (*h).tag != ObjTag::Coro {
                return 0;
            }
            if (*h).tag == ObjTag::Coro {
                return self.thread_work(&*(h as *const crate::runtime::Coro), run);
            }
            if matches!(d, Dialect::L51 | Dialect::L52 | Dialect::L53) {
                return match (*h).tag {
                    ObjTag::Table => {
                        let t = &*(h as *const Table);
                        if self.is_own_table(t) {
                            return 0;
                        }
                        // the dummy node counts as one
                        let nodes = t.node_count().max(1);
                        (self.sizes.table
                            + self.array_bytes(t.asize as usize)
                            + self.sizes.node * nodes) as i64
                    }
                    // 5.1 marks a userdata black at once, 5.2 and 5.3 count
                    // it as they mark it
                    ObjTag::Str | ObjTag::Userdata | ObjTag::Upvalue | ObjTag::Ghost => 0,
                    _ => self.puc_objsize(h) as i64,
                };
            }
            let p = |b: u32, e: usize| (b as usize).checked_div(e).unwrap_or(0) as i64;
            match (*h).tag {
                ObjTag::Table => {
                    let t = &*(h as *const Table);
                    if self.is_own_table(t) {
                        return 0;
                    }
                    let nodes = t.node_count() as i64;
                    if d == Dialect::L54 {
                        1 + i64::from(t.alimit.get()) + 2 * nodes
                    } else {
                        1 + 2 * nodes.max(1) + i64::from(t.asize)
                    }
                }
                ObjTag::Closure => {
                    let c = &*(h as *const LuaClosure);
                    1 + i64::from(c.upvals_len)
                }
                ObjTag::Native => 1 + native_upvals(h) as i64,
                ObjTag::Proto => {
                    let a = (*(h as *const Proto)).puc_arrays.get();
                    let s = self.sizes;
                    1 + p(a[2], s.tvalue)
                        + p(a[6], s.upvaldesc)
                        + p(a[1], s.ptr)
                        + p(a[5], s.locvar)
                }
                ObjTag::Coro => 0,
                // a userdata without user values is marked black at once
                ObjTag::Userdata => 0,
                ObjTag::Str | ObjTag::Upvalue | ObjTag::Ghost => 0,
            }
        }
    }

    /// The work of traversing a thread: up to 5.3 its size, 5.4 one slot
    /// for each of its stack's, 5.5 for each in use.
    fn thread_work(&self, c: &crate::runtime::Coro, run: Option<(u32, u32, u32)>) -> i64 {
        let d = self.table_dialect;
        let ci = if d == Dialect::L51 {
            c.frame_size
        } else {
            c.ci_alloc
        };
        let (top, size, ci) = run.unwrap_or((c.top, c.stack_size, ci));
        match d {
            Dialect::L54 => 1 + i64::from(size),
            Dialect::L55 => 1 + i64::from(top),
            _ => self.thread_bytes(size, ci) as i64,
        }
    }

    /// PUC's size of a thread with a stack of `stack` slots and `ci`
    /// `CallInfo`s (5.1: the size of its array).
    pub(crate) fn thread_bytes(&self, stack: u32, ci: u32) -> usize {
        thread_bytes(&self.sizes, self.table_dialect, stack, ci)
    }
}

/// PUC's size of the object behind `h` other than a thread, in dialect `d`
/// with `own` luna's own tables; 0 for an object of luna's own.
///
/// # Safety
/// `h` heads an allocated object other than a thread.
pub(in crate::runtime::heap) unsafe fn objsize(
    s: &PucSizes,
    d: Dialect,
    own: &[*const Table],
    h: *mut GcHeader,
) -> usize {
    // SAFETY: the caller's contract; the tag names the type
    unsafe {
        if is_uncounted(h) {
            return 0;
        }
        match (*h).tag {
            ObjTag::Table => {
                let t = h as *const Table;
                if own.contains(&t) {
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
                let env = (*h).sub & acct::ENV51 != 0;
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
                    f if f & acct::FIXED_EXT != 0 => s.lngstr_hdr,
                    _ => s.string(st.len(), !st.is_short() && d == Dialect::L55),
                }
            }
            ObjTag::Coro | ObjTag::Ghost => 0,
        }
    }
}

/// The upvalues of PUC's C function standing for the native behind `h`.
///
/// # Safety
/// `h` heads an allocated native function.
unsafe fn native_upvals(h: *mut GcHeader) -> usize {
    // SAFETY: the caller's contract
    unsafe {
        match (*h).sub & !UNCOUNTED {
            0 => (&*(h as *const NativeClosure)).upvals.len(),
            k => k as usize - 1,
        }
    }
}

/// The bytes of a table's array part of `n` slots.
pub(in crate::runtime::heap) fn array_bytes(s: &PucSizes, d: Dialect, n: usize) -> usize {
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
pub(in crate::runtime::heap) fn hash_bytes(s: &PucSizes, d: Dialect, h: usize) -> usize {
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
pub(in crate::runtime::heap) fn thread_bytes(
    s: &PucSizes,
    d: Dialect,
    stack: u32,
    ci: u32,
) -> usize {
    s.thread + stack_bytes(s, d, stack) + s.callinfo * ci as usize
}

/// The bytes of a stack of `n` slots (PUC's `stacksize`, which from 5.4
/// leaves out the `EXTRA_STACK` slots it allocates beyond it).
pub(in crate::runtime::heap) fn stack_bytes(s: &PucSizes, d: Dialect, n: u32) -> usize {
    match (n, d >= Dialect::L54) {
        (0, _) => 0,
        (n, true) => (n as usize + 5) * s.tvalue,
        (n, false) => n as usize * s.tvalue,
    }
}
