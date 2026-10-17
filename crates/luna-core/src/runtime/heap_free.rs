//! Freeing objects.

use super::*;

impl Heap {
    /// Free the object behind `h` and take its bytes off the count. The
    /// caller unlinks it (or drops the whole list) itself.
    ///
    /// # Safety
    /// `h` heads a live object of this heap; the caller has read what it
    /// needs from it (its `next` link) and nothing uses it afterwards.
    pub(super) unsafe fn free_obj(&mut self, h: *mut GcHeader) {
        #[cfg(feature = "gc-verify")]
        {
            self.recently_freed.insert(h as usize);
            crate::runtime::gc_verify_probe::FREED.with(|f| f.borrow_mut().insert(h as usize));
            // SAFETY: the caller's contract: `h` is still allocated
            if unsafe { (*h).tag } == ObjTag::Str {
                // SAFETY: as above, a string
                let b = unsafe { Gc::from_ptr(h as *mut LuaStr) }
                    .as_bytes()
                    .to_vec();
                crate::runtime::gc_verify_probe::FREED_STR
                    .with(|f| f.borrow_mut().insert(h as usize, b));
            }
        }
        // SAFETY: the caller's contract: `h` heads an unlinked object of this heap that nothing uses afterwards; its tag names the type it was boxed as (`adopt`, `new_table`, `alloc_str`), so each arm frees it with the matching layout; a short string stays in the string table's chains until removed here
        unsafe {
            if (*h).cls & STANDS != 0 {
                self.free_stand_in(h);
            }
            match (*h).tag {
                ObjTag::Table => {
                    let t = h as *mut Table;
                    if let Some(i) = self.puc.own_tables.iter().position(|&o| std::ptr::eq(o, t)) {
                        self.puc.own_tables.swap_remove(i);
                    } else {
                        self.acct_table_free((*t).asize as usize, (*t).node_count());
                    }
                    // pool recycle. Drop the
                    // Box-owned interior (slab, nodes, metatable) so the
                    // Table struct itself can be re-handed-out by a
                    // future `new_table` without re-mallocing. Cap pool
                    // at 4096 entries to bound idle memory.
                    const TABLE_POOL_CAP: usize = 4096;
                    (*t).free_parts(self.mem());
                    // the pool owns the table's block until reuse or the
                    // heap's drop; a pool that cannot grow frees it instead
                    if self.table_pool.len() < TABLE_POOL_CAP
                        && self.table_pool.try_reserve(1).is_ok()
                    {
                        self.table_pool.push(std::ptr::NonNull::new_unchecked(t));
                        (*t).metatable = None;
                    } else {
                        self.free_block(t);
                    }
                }
                ObjTag::Proto => {
                    if !is_uncounted(h) {
                        self.acct_proto_free(&*(h as *mut Proto));
                    }
                    self.free_block(h as *mut Proto);
                }
                ObjTag::Closure => {
                    let c = &*(h as *mut LuaClosure);
                    if !is_uncounted(h) {
                        // the prototype may be freed already
                        let env = (*h).sub & super::acct::ENV51 != 0;
                        let n = c.upvals_len as usize - usize::from(env);
                        self.acct_free(self.sizes.lua_closure(n));
                    }
                    (*(h as *mut LuaClosure)).free_overflow(self.mem());
                    self.free_block(h as *mut LuaClosure);
                }
                ObjTag::Upvalue => {
                    if !is_uncounted(h) {
                        self.acct_free(self.sizes.upval);
                    }
                    self.free_block(h as *mut Upvalue);
                }
                ObjTag::Native => {
                    if is_uncounted(h) {
                        self.free_stand_in(h);
                    }
                    let nat = &*(h as *mut NativeClosure);
                    // the upvalues PUC's function has, when luna keeps others
                    let n = match (*h).sub & !UNCOUNTED {
                        0 => nat.upvals.len(),
                        k => k as usize - 1,
                    };
                    if (n > 0 || self.table_dialect == crate::runtime::table::Dialect::L51)
                        && !is_uncounted(h)
                    {
                        self.acct_free(self.sizes.c_closure(n));
                    }
                    self.free_block(h as *mut NativeClosure);
                }
                ObjTag::Coro => {
                    if !is_uncounted(h) {
                        let c = &*(h as *mut crate::runtime::Coro);
                        let ci = if self.table_dialect == crate::runtime::table::Dialect::L51 {
                            c.frame_size
                        } else {
                            c.ci_alloc
                        };
                        self.acct_thread_free(c.stack_size, ci);
                    }
                    self.free_block(h as *mut crate::runtime::Coro);
                }
                ObjTag::Userdata => {
                    if !is_uncounted(h) {
                        self.acct_free((*(h as *mut Userdata)).puc_size);
                    } else {
                        self.free_stand_in(h);
                    }
                    self.free_block(h as *mut Userdata);
                }
                ObjTag::Ghost => self.free_ghost(h as *mut Ghost),
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    if (*h).sub & EXTERNAL != 0 {
                        // its block goes back first, then its header
                        self.trace_raw((*s).len() + 1, 0);
                        self.acct_free(self.sizes.lngstr_hdr + 2 * self.sizes.ptr);
                    } else if (*h).sub & acct::FIXED_EXT != 0 {
                        self.acct_free(self.sizes.lngstr_hdr);
                    } else if (*h).sub & strt::SWEPT != 0 {
                        // the string sweep took it off the count
                    } else if (*h).sub & UNCOUNTED != 0 {
                        self.free_stand_in(h);
                    } else {
                        return self.free_plain_str(s);
                    }
                    if (*s).is_short() {
                        self.strings.remove(s);
                    }
                    string::free(s, self.mem());
                }
            }
        }
    }

    /// Whether the object behind `h` is a string PUC counts as luna does,
    /// one [`Heap::free_plain_str`] frees.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    #[inline(always)]
    pub(super) unsafe fn is_plain_str(h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract; only the header is read
        unsafe {
            (*h).tag == ObjTag::Str
                && (*h).sub & (EXTERNAL | acct::FIXED_EXT | strt::SWEPT | UNCOUNTED) == 0
        }
    }

    /// [`Heap::free_obj`] of a string [`Heap::is_plain_str`] accepts.
    ///
    /// # Safety
    /// As for `free_obj`, and `s` is such a string.
    pub(super) unsafe fn free_plain_str(&mut self, s: *mut LuaStr) {
        // SAFETY: the caller's contract: `s` is an unlinked string of this heap nothing uses afterwards; a short one is still in the string table
        unsafe {
            #[cfg(feature = "gc-trace")]
            if self.trace.where_ {
                let b = String::from_utf8_lossy(string::bytes_of(s)).into_owned();
                self.trace.line(format_args!("s {b:?}"));
            }
            self.acct_string_free((*s).len());
            if (*s).is_short() {
                self.strings.remove(s);
            }
            string::free(s, self.mem());
        }
    }

    /// The objects of PUC's that `h`, an uncounted object of luna's, stood
    /// for (`PucState::stand_ins`) are freed with it.
    fn free_stand_in(&mut self, h: *mut GcHeader) {
        while let Some(i) = self.puc.stand_ins.iter().position(|(p, _)| *p == h) {
            let (_, sizes) = self.puc.stand_ins.swap_remove(i);
            for s in sizes {
                self.acct_free(s);
            }
        }
    }
}
