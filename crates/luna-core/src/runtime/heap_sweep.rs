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
        if self.sweep_cur.is_null() {
            self.phase = GcPhase::Pause;
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
                    let internal = (*t).internal_bytes();
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<Table>() + internal);
                    // pool recycle. Drop the
                    // Box-owned interior (slab, nodes, metatable) so the
                    // Table struct itself can be re-handed-out by a
                    // future `new_table` without re-mallocing. Cap pool
                    // at 4096 entries to bound idle memory.
                    const TABLE_POOL_CAP: usize = 4096;
                    if self.table_pool.len() < TABLE_POOL_CAP {
                        // Free interior heap allocations now; an empty Box is
                        // dangling, so reassigning is just a pointer move.
                        (*t).drop_array_part();
                        (*t).drop_hash_part();
                        (*t).metatable = None;
                        // Stash the raw pointer for future reuse.
                        // SAFETY: t is non-null (came from a live Gc<Table>);
                        // pool owns it until reuse or Heap::Drop.
                        self.table_pool.push(std::ptr::NonNull::new_unchecked(t));
                    } else {
                        drop(Box::from_raw(t));
                    }
                }
                ObjTag::Proto => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<Proto>());
                    drop(Box::from_raw(h as *mut Proto));
                }
                ObjTag::Closure => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<LuaClosure>());
                    drop(Box::from_raw(h as *mut LuaClosure));
                }
                ObjTag::Upvalue => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<Upvalue>());
                    drop(Box::from_raw(h as *mut Upvalue));
                }
                ObjTag::Native => {
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<NativeClosure>());
                    drop(Box::from_raw(h as *mut NativeClosure));
                }
                ObjTag::Coro => {
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<crate::runtime::Coro>());
                    drop(Box::from_raw(h as *mut crate::runtime::Coro));
                }
                ObjTag::Userdata => {
                    let extra = (*(h as *mut Userdata)).extra_bytes;
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<Userdata>() + extra);
                    drop(Box::from_raw(h as *mut Userdata));
                }
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    self.bytes = self.bytes.saturating_sub(string::alloc_size((*s).len()));
                    if (*s).is_short() {
                        self.strings.remove(s);
                    }
                    string::free(s);
                }
            }
        }
    }
}
