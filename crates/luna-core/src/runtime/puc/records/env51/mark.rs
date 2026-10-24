//! Marking 5.1's environment records, as PUC marks the tables and
//! closures they stand for.

use super::*;

impl Marker {
    /// Mark the environment of the object behind `h`, which its class
    /// bits `cls` name; `h` itself is only compared.
    pub(crate) fn mark_env51(&mut self, h: *mut GcHeader, cls: u8) {
        let env = cls >> ENV_SHIFT;
        if env == 0 || env == ENV_PROTO {
            return;
        }
        let Some(e) = self.lent.env51.as_deref() else {
            return;
        };
        let g = e.gen_of(h);
        let h = match env {
            ENV_PACKAGE => e.package,
            ENV_IO => &g.ph[E1].hdr as *const GcHeader as *mut GcHeader,
            ENV_IO_STD => &g.ph[E2].hdr as *const GcHeader as *mut GcHeader,
            _ => &g.ph[E3].hdr as *const GcHeader as *mut GcHeader,
        };
        if !h.is_null() {
            // SAFETY: a record (boxed in the records the marker holds, which
            // do not move), or the live package table
            unsafe { self.header(h) };
        }
    }

    /// Traverse the record `h`, as PUC traverses its table or closure:
    /// E1's array from its end, the default output then input, then its
    /// one node's value; a closure's environment.
    ///
    /// # Safety
    /// `h` heads a record of the marker's.
    pub(in crate::runtime::puc) unsafe fn trace_phantom(&mut self, h: *mut GcHeader) {
        // what the record holds, gathered before any is marked
        let mut kids = [ptr::null_mut(); 8];
        let mut n = 0;
        // SAFETY: the caller's contract; the records the marker holds are
        // boxed, so their headers stay where they are while it marks
        unsafe {
            let p = &*(h as *const Phantom);
            let Some(e) = self.lent.env51.as_deref() else {
                return;
            };
            let Some(g) = e
                .gens
                .iter()
                .find(|g| g.ph.iter().any(|q| std::ptr::eq(q, p)))
            else {
                return;
            };
            let mut push = |k: *mut GcHeader| {
                kids[n] = k;
                n += 1;
            };
            if std::ptr::eq(p, &g.ph[E1]) {
                for f in [g.io[1], g.io[0]] {
                    if !f.is_null() {
                        push(f);
                    }
                }
            }
            // a table's one node: its key, then its value (`kids`)
            if p.kids[0] != NONE && !e.close.is_null() {
                push(e.close);
            }
            if p.env != 0 {
                // a closure's environment, its own opening's E1
                push(&g.ph[E1].hdr as *const GcHeader as *mut GcHeader);
            }
            for &k in &p.kids {
                if k != NONE {
                    push(&g.ph[usize::from(k)].hdr as *const GcHeader as *mut GcHeader);
                }
            }
            for &k in &kids[..n] {
                self.header(k);
            }
        }
    }
}

impl Heap {
    /// `lua_setfield` of `key` and the record `v` (a closure) into the
    /// record `t`, one of the io library's tables: 5.1's `luaC_barriert`.
    ///
    /// # Safety
    /// `t` and `v` head records of this heap's.
    pub(crate) unsafe fn env51_set_key(
        &mut self,
        t: *mut GcHeader,
        key: Gc<LuaStr>,
        v: *mut GcHeader,
    ) {
        self.env51_records().close = key.as_ptr() as *mut GcHeader;
        // SAFETY: the caller's contract; records live as long as the heap,
        // and only flags are read
        unsafe {
            if is_white((*v).flags()) {
                self.barrier_back_obj(t);
            }
        }
    }
}
