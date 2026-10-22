//! Marking 5.1's environment records, as PUC marks the tables and
//! closures they stand for.

use super::*;

impl Marker {
    /// Mark the environment of the object behind `h`, which its class
    /// bits `cls` name.
    pub(crate) fn mark_env51(&mut self, h: *mut GcHeader, cls: u8) {
        let env = cls >> ENV_SHIFT;
        if env == 0 || env == ENV_PROTO || self.env51.is_null() {
            return;
        }
        // SAFETY: the records live as long as the heap the marker marks
        let e = unsafe { &*self.env51 };
        let g = e.gen_of(h);
        let h = match env {
            ENV_PACKAGE => e.package,
            ENV_IO => &g.ph[E1].hdr as *const GcHeader as *mut GcHeader,
            ENV_IO_STD => &g.ph[E2].hdr as *const GcHeader as *mut GcHeader,
            _ => &g.ph[E3].hdr as *const GcHeader as *mut GcHeader,
        };
        if !h.is_null() {
            // SAFETY: a record, or the live package table
            unsafe { self.header(h) };
        }
    }

    /// Traverse the record `h`, as PUC traverses its table or closure:
    /// E1's array from its end, the default output then input, then its
    /// one node's value; a closure's environment.
    ///
    /// # Safety
    /// `h` heads a record of the marker's.
    pub(crate) unsafe fn trace_phantom(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; the records outlive the marking
        unsafe {
            let p = &*(h as *const Phantom);
            let e = &*self.env51;
            let Some(g) = e
                .gens
                .iter()
                .find(|g| g.ph.iter().any(|q| std::ptr::eq(q, p)))
            else {
                return;
            };
            if std::ptr::eq(p, &g.ph[E1]) {
                for f in [g.io[1], g.io[0]] {
                    if !f.is_null() {
                        self.header(f);
                    }
                }
            }
            // a table's one node: its key, then its value (`kids`)
            if p.kids[0] != NONE && !e.close.is_null() {
                self.header(e.close);
            }
            if p.env != 0 {
                // a closure's environment, its own opening's E1
                let e1 = &g.ph[E1].hdr as *const GcHeader as *mut GcHeader;
                self.header(e1);
            }
            for &k in &p.kids {
                if k != NONE {
                    self.header(&g.ph[usize::from(k)].hdr as *const GcHeader as *mut GcHeader);
                }
            }
        }
    }
}

impl Heap {
    /// The pointer a marker reaches the records through.
    pub(in crate::runtime) fn env51_ptr(&self) -> *const Env51 {
        self.env51_box
            .as_deref()
            .map_or(ptr::null(), |e| e as *const Env51)
    }

    /// `lua_setfield` of `key` and the record `v` (a closure) into the
    /// record `t`, one of the io library's tables: 5.1's `luaC_barriert`.
    pub(crate) fn env51_set_key(&mut self, t: *mut GcHeader, key: Gc<LuaStr>, v: *mut GcHeader) {
        self.env51_records().close = key.as_ptr() as *mut GcHeader;
        // SAFETY: records live as long as the heap; only flags are read
        unsafe {
            if is_white((*v).flags) {
                self.barrier_back_obj(t);
            }
        }
    }
}
