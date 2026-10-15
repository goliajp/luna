//! Sweeping and freeing objects.

use super::*;

impl Heap {
    /// Sweep the whole `all` list in one pass: free dead-white objects,
    /// transition survivors (BLACK or current-white) to the new current-white
    /// so the next cycle can re-mark them. Returns the number of objects freed.
    pub(super) fn full_sweep(&mut self) -> usize {
        // detach the list first so freeing (which needs &mut self for the
        // string table) never aliases a pointer into self
        let mut freed = 0;
        let new_white = self.current_white;
        // SAFETY: the detached list holds only objects this heap allocated and has not freed; `link` points at `head` or at the `next` field of a survivor, and a dead object is unlinked before `free_obj` and not touched after it
        unsafe {
            // PUC `sweeplist`: `link` is the field that points at `cur`, so
            // a survivor costs one store (its color) and only a freed object
            // relinks its predecessor
            let mut head = std::mem::replace(&mut self.all, ptr::null_mut());
            let mut link: *mut *mut GcHeader = ptr::addr_of_mut!(head);
            while !(*link).is_null() {
                let cur = *link;
                let f = (*cur).flags;
                // dead = other-white (i.e. white but not current-white).
                // Survivors are BLACK (just-marked) or current-white (born
                // during the sweep itself).
                if is_white(f) && (f & new_white) == 0 {
                    *link = (*cur).next;
                    self.free_obj(cur);
                    freed += 1;
                } else {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    link = ptr::addr_of_mut!((*cur).next);
                }
            }
            self.all = head;
        }
        self.live -= freed;
        // a sweep keeps the order of `all`, so the young boundary (marked,
        // so it survived) still splits new objects from old ones
        #[cfg(feature = "gc-verify")]
        self.verify_no_dangling("full_sweep");
        #[cfg(any(debug_assertions, feature = "gc-verify"))]
        self.verify_slow_bits("full_sweep");
        freed
    }

    /// Sweep up to `budget` objects from the detached `sweep_cur` list: free
    /// unmarked ones, splice marked survivors back onto `all` (clearing their
    /// MARK bit). Returns true once the list is exhausted (cycle complete →
    /// back to `Pause`). Safe with no write barrier: marking was atomic, so any
    /// object still unmarked here was unreachable at mark time and the mutator
    /// holds no reference that could have resurrected it.
    pub(crate) fn gc_sweep_step(&mut self, budget: usize) -> bool {
        let mut n = 0;
        let new_white = self.current_white;
        // SAFETY: `sweep_cur` is the list detached at the atomic step and holds only objects this heap allocated and has not freed; `next` is read before `free_obj`, and a survivor is relinked onto `all` before the walk moves on
        unsafe {
            while n < budget && !self.sweep_cur.is_null() {
                let cur = self.sweep_cur;
                let next = (*cur).next;
                let f = (*cur).flags;
                let dead = is_white(f) && (f & new_white) == 0;
                if !dead {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    (*cur).next = self.all;
                    self.all = cur;
                } else {
                    self.free_obj(cur);
                    self.live -= 1;
                }
                self.sweep_cur = next;
                n += 1;
            }
        }
        // Verify after EVERY step, not just cycle completion: the
        // mutator runs between steps, so a reference dangling mid-cycle
        // is already a live bug.
        #[cfg(feature = "gc-verify")]
        self.verify_no_dangling("sweep_step");
        // survivors went to the head of `all`, in front of the young
        // boundary: an emergency collection marks them as young, which only
        // keeps them longer
        if self.sweep_cur.is_null() {
            self.phase = GcPhase::Pause;
            self.mem_ctx().gc_cycle_ended(self.bytes);
            #[cfg(any(debug_assertions, feature = "gc-verify"))]
            self.verify_slow_bits("sweep_step");
            true
        } else {
            false
        }
    }

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
        }
        // SAFETY: the caller's contract: `h` heads an unlinked object of this heap that nothing uses afterwards; its tag names the type it was boxed as (`adopt`, `new_table`, `alloc_str`), so each arm frees it with the matching layout; a short string stays in the string table's chains until removed here
        unsafe {
            match (*h).tag {
                ObjTag::Table => {
                    let t = h as *mut Table;
                    self.acct_table_free((*t).asize as usize, (*t).node_count());
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
                        && self
                            .table_pool
                            .push(std::ptr::NonNull::new_unchecked(t))
                            .is_ok()
                    {
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
                    let nat = &*(h as *mut NativeClosure);
                    let n = nat.upvals.len();
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
                    let extra = (*(h as *mut Userdata)).extra_bytes;
                    if !is_uncounted(h) {
                        self.acct_free(self.sizes.userdata(1, 0) + extra);
                    }
                    self.free_block(h as *mut Userdata);
                }
                ObjTag::Ghost => self.free_ghost(h as *mut ghost::Ghost),
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    if (*h).sub & EXTERNAL != 0 {
                        // its block goes back first, then its header
                        self.trace_raw((*s).len() + 1, 0);
                        self.acct_free(self.sizes.lngstr_hdr + 2 * self.sizes.ptr);
                    } else if (*h).sub & UNCOUNTED != 0 {
                        if let Some(i) = self.puc.stand_ins.iter().position(|&(p, _)| p == h) {
                            let (_, size) = self.puc.stand_ins.swap_remove(i);
                            self.acct_free(size);
                        }
                    } else {
                        self.acct_string_free((*s).len());
                    }
                    if (*s).is_short() {
                        self.strings.remove(s);
                    }
                    string::free(s, self.mem());
                }
            }
        }
    }
}
