//! What PUC's collector counts for an object the heap frees or traverses:
//! its size, and the work of traversing it (`propagatemark`'s result), by
//! dialect.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::work::{native_upvals, objsize, thread_bytes};

impl Heap {
    /// PUC's size of the object behind `h`, as the count has it; 0 for an
    /// object of luna's own.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(in crate::runtime) unsafe fn puc_objsize(&self, h: *mut GcHeader) -> usize {
        // SAFETY: the caller's contract
        unsafe {
            if (*h).tag == ObjTag::Coro {
                if (*h).is_uncounted() {
                    return 0;
                }
                let c = &*(h as *const crate::runtime::Coro);
                let ci = if self.puc.dialect == Dialect::L51 {
                    c.frame_size
                } else {
                    c.ci_alloc
                };
                return self.thread_bytes(c.stack_size, ci);
            }
            objsize(&self.puc.sizes, self.puc.dialect, h)
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
    pub(in crate::runtime) unsafe fn object_work(&self, h: *mut GcHeader, run: Running) -> i64 {
        let run = if h == run.h {
            Some((run.top, run.size, run.ci))
        } else if h == run.main {
            Some((run.main_top, run.main_size, run.main_ci))
        } else {
            None
        };
        // SAFETY: the caller's contract
        unsafe {
            let d = self.puc.dialect;
            // a function of PUC's C closures, the library's too (made
            // without counting), at PUC's number of upvalues; from 5.2 on
            // one with none is a light function, no object
            if (*h).tag == ObjTag::Native {
                let n = native_upvals(h);
                return match d {
                    Dialect::L51 => self.puc.sizes.c_closure(n) as i64,
                    _ if n == 0 => 0,
                    Dialect::L52 | Dialect::L53 => self.puc.sizes.c_closure(n) as i64,
                    _ => 1 + n as i64,
                };
            }
            // the main thread is luna's own, with a stack PUC has
            if (*h).is_uncounted() && (*h).tag != ObjTag::Coro {
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
                        // the dummy node counts as one, but in 5.3
                        // (`allocsizenode`)
                        let nodes = match d {
                            Dialect::L53 => t.node_count(),
                            _ => t.node_count().max(1),
                        };
                        (self.puc.sizes.table
                            + self.array_bytes(t.asize as usize)
                            + self.puc.sizes.node * nodes) as i64
                    }
                    // 5.1 marks a userdata black at once, 5.2 and 5.3 count
                    // it as they mark it
                    ObjTag::Ghost if crate::runtime::puc::records::env51::is_phantom(h) => {
                        crate::runtime::puc::records::env51::phantom_size(h)
                    }
                    ObjTag::Ghost if crate::runtime::puc::records::parse::is_load_tab(h) => {
                        self.load_tab_work(h)
                    }
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
                    let s = self.puc.sizes;
                    1 + p(a[2], s.tvalue)
                        + p(a[6], s.upvaldesc)
                        + p(a[1], s.ptr)
                        + p(a[5], s.locvar)
                }
                ObjTag::Coro => 0,
                // a userdata without user values is marked black at once
                ObjTag::Userdata => 0,
                ObjTag::Ghost if crate::runtime::puc::records::parse::is_load_tab(h) => {
                    self.load_tab_work(h)
                }
                ObjTag::Str | ObjTag::Upvalue | ObjTag::Ghost => 0,
            }
        }
    }

    /// The work of traversing a thread: up to 5.3 its size, 5.4 one slot
    /// for each of its stack's, 5.5 for each in use.
    fn thread_work(&self, c: &crate::runtime::Coro, run: Option<(u32, u32, u32)>) -> i64 {
        let d = self.puc.dialect;
        let ci = if d == Dialect::L51 {
            c.frame_size
        } else {
            c.ci_alloc
        };
        let (top, size, ci) = run.unwrap_or((c.top, c.stack_size, ci));
        match d {
            Dialect::L54 => 1 + i64::from(size),
            // PUC's top is luna's plus one (its slot 0 holds no value of luna's)
            Dialect::L55 => 2 + i64::from(top),
            // 5.3 counts `lua_State`, without the extra space before it
            Dialect::L53 => (self.thread_bytes(size, ci) - self.puc.sizes.ptr) as i64,
            _ => self.thread_bytes(size, ci) as i64,
        }
    }

    /// PUC's size of a thread with a stack of `stack` slots and `ci`
    /// `CallInfo`s (5.1: the size of its array).
    pub(in crate::runtime::puc) fn thread_bytes(&self, stack: u32, ci: u32) -> usize {
        thread_bytes(&self.puc.sizes, self.puc.dialect, stack, ci)
    }
}
