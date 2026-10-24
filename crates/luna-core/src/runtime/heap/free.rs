//! Freeing objects.

use super::*;

impl Heap {
    /// Free the object behind `h` and take its bytes off the count. The
    /// caller unlinks it (or drops the whole list) itself.
    ///
    /// # Safety
    /// `h` heads a live object of this heap; the caller has read what it
    /// needs from it (its `next` link) and nothing uses it afterwards.
    pub(in crate::runtime) unsafe fn free_obj(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract: `h` heads an unlinked object of this heap that nothing uses afterwards; its tag names the type it was boxed as (`adopt`, `new_table`, `alloc_str`), so each arm frees it with the matching layout; a short string stays in the string table's chains until removed here
        unsafe {
            self.hook_freed(Gc::from_ptr_unchecked(h));
            if (*h).cls & STANDS != 0 {
                self.free_stand_in(h);
            }
            match (*h).tag {
                ObjTag::Table => {
                    let t = h as *mut Table;
                    if (*h).cls & UNLISTED == 0 {
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
                    if !(*h).is_uncounted() {
                        self.acct_proto_free(&*(h as *mut Proto));
                    }
                    self.free_block(h as *mut Proto);
                }
                ObjTag::Closure => {
                    let c = &*(h as *mut LuaClosure);
                    if !(*h).is_uncounted() {
                        // the prototype may be freed already
                        let env = (*h).sub & crate::runtime::puc::ENV51 != 0;
                        let n = c.upvals_len as usize - usize::from(env);
                        self.acct_part(self.puc_size(PucObj::LuaClosure(n)), 0);
                    }
                    (*(h as *mut LuaClosure)).free_overflow(self.mem());
                    self.free_block(h as *mut LuaClosure);
                }
                ObjTag::Upvalue => {
                    if !(*h).is_uncounted() {
                        self.acct_part(self.puc_size(PucObj::Upval), 0);
                    }
                    self.free_block(h as *mut Upvalue);
                }
                ObjTag::Native => {
                    if (*h).is_uncounted() {
                        self.free_stand_in(h);
                    }
                    let nat = &*(h as *mut NativeClosure);
                    // the upvalues PUC's function has, when luna keeps others
                    let n = match (*h).sub & !UNCOUNTED {
                        0 => nat.upvals.len(),
                        k => k as usize - 1,
                    };
                    if self.puc_c_function_is_object(n) && !(*h).is_uncounted() {
                        self.acct_part(self.puc_size(PucObj::CClosure(n)), 0);
                    }
                    if (*h).cls & SLAB != 0 {
                        ptr::drop_in_place(h as *mut NativeClosure);
                        self.slab_free(h as *mut NativeClosure);
                    } else {
                        self.free_block(h as *mut NativeClosure);
                    }
                }
                ObjTag::Coro => {
                    if !(*h).is_uncounted() {
                        let c = &*(h as *mut crate::runtime::Coro);
                        let ci = self.puc_thread_ci(c);
                        self.acct_thread_free(c.stack_size, ci);
                    }
                    self.free_block(h as *mut crate::runtime::Coro);
                }
                ObjTag::Userdata => {
                    if h == self.stdin_ud {
                        self.stdin_ud = ptr::null_mut();
                    }
                    crate::vm::lib_io::write_out_freed(Gc::from_ptr(h as *mut Userdata));
                    if !(*h).is_uncounted() {
                        self.acct_part((*(h as *mut Userdata)).puc_size, 0);
                    } else {
                        self.free_stand_in(h);
                    }
                    self.free_block(h as *mut Userdata);
                }
                ObjTag::Ghost if super::parse::is_load_tab(h) => self.free_load_tab(h),
                ObjTag::Ghost => self.free_ghost(h as *mut Ghost),
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    if (*h).sub & EXTERNAL != 0 {
                        // its block goes back first, then its header
                        self.trace_raw((*s).len() + 1, 0);
                        self.acct_part(self.puc_size(PucObj::ExternalStr), 0);
                    } else if (*h).sub & puc::FIXED_EXT != 0 {
                        self.acct_part(self.puc_size(PucObj::LongStrHeader), 0);
                    } else if (*h).sub & strt::SWEPT != 0 {
                        // the string sweep took it off the count
                    } else if (*h).sub & UNCOUNTED != 0 {
                        self.free_stand_in(h);
                    } else {
                        return self.free_plain_str(s);
                    }
                    if (*s).is_short() && !self.dropping {
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
                && (*h).sub & (EXTERNAL | puc::FIXED_EXT | strt::SWEPT | UNCOUNTED) == 0
        }
    }

    /// [`Heap::free_obj`] of a string [`Heap::is_plain_str`] accepts.
    ///
    /// # Safety
    /// As for `free_obj`, and `s` is such a string.
    pub(super) unsafe fn free_plain_str(&mut self, s: *mut LuaStr) {
        // SAFETY: the caller's contract: `s` is an unlinked string of this heap nothing uses afterwards; a short one is still in the string table
        unsafe {
            self.hook_string_freed(Gc::from_ptr_unchecked(s));
            if !self.dropping {
                self.acct_string_free((*s).len());
                if (*s).is_short() {
                    self.strings.remove(s);
                }
            }
            string::free(s, self.mem());
        }
    }
}
