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
            match (*h).tag {
                ObjTag::Table => {
                    let t = h as *mut Table;
                    self.puc_freed(&*t);
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
                    self.puc_freed(&*(h as *const Proto));
                    self.free_block(h as *mut Proto);
                }
                ObjTag::Closure => {
                    self.puc_freed(&*(h as *const LuaClosure));
                    // the others' upvalues are objects, maybe freed already
                    if self.puc_rules().upvals_refcounted {
                        for &uv in (*(h as *const LuaClosure)).upvals() {
                            self.upval_let_go(uv.as_ptr());
                        }
                    }
                    (*(h as *mut LuaClosure)).free_overflow(self.mem());
                    self.free_block(h as *mut LuaClosure);
                }
                ObjTag::Upvalue => {
                    // one still open off its thread's list (PUC `freeupval`),
                    // that thread not freed yet: freeing one closes these
                    // (when the heap is dropped, lists and threads go anyway)
                    if let UpvalState::Open {
                        thread: Some(co), ..
                    } = (*(h as *const Upvalue)).state
                        && !self.dropping
                    {
                        let uv = h as *mut Upvalue;
                        co.as_mut().open_upvals.retain(|&(_, u)| u.as_ptr() != uv);
                    }
                    self.puc_freed(&*(h as *const Upvalue));
                    self.free_block(h as *mut Upvalue);
                }
                ObjTag::Native => {
                    self.puc_freed(&*(h as *const NativeClosure));
                    if (*h).cls & SLAB != 0 {
                        ptr::drop_in_place(h as *mut NativeClosure);
                        self.slab_free(h as *mut NativeClosure);
                    } else {
                        self.free_block(h as *mut NativeClosure);
                    }
                }
                ObjTag::Coro => {
                    self.upvals_of_freed_thread(h as *mut crate::runtime::Coro);
                    self.puc_freed(&*(h as *const crate::runtime::Coro));
                    self.free_block(h as *mut crate::runtime::Coro);
                }
                ObjTag::Userdata => {
                    if h == self.weak_userdata {
                        self.weak_userdata = ptr::null_mut();
                    }
                    if let Some(f) = self.userdata_freed {
                        f(Gc::from_ptr(h as *mut Userdata));
                    }
                    self.puc_freed(&*(h as *const Userdata));
                    self.free_block(h as *mut Userdata);
                }
                // a record of the model's, which frees it whole
                ObjTag::Ghost => self.puc_free_record(h),
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    if Self::is_plain_str(h) {
                        return self.free_plain_str(s);
                    }
                    self.puc_freed(&*s);
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
                && (*h).sub & (EXTERNAL | puc::FIXED_EXT | model::SWEPT | UNCOUNTED) == 0
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
            self.puc_freed(&*s);
            if !self.dropping && (*s).is_short() {
                self.strings.remove(s);
            }
            string::free(s, self.mem());
        }
    }
}
