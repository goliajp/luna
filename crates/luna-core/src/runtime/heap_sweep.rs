//! Sweeping and freeing objects.

use super::*;
use crate::runtime::table::Dialect;

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

    /// Sweep up to `budget` objects in place (PUC `sweeplist`): free the
    /// dead ones, turn the survivors to the current white, keeping their
    /// order. Returns true once the list is done (the cycle is back at
    /// `Pause`). Safe with no write barrier: marking was atomic, so an
    /// object still unmarked here was unreachable then, and the mutator
    /// holds no reference that could have brought it back.
    pub(crate) fn gc_sweep_step(&mut self, budget: usize) -> bool {
        self.sweep_objects(budget, false);
        // Verify after EVERY step, not just cycle completion: the
        // mutator runs between steps, so a reference dangling mid-cycle
        // is already a live bug.
        #[cfg(feature = "gc-verify")]
        self.verify_no_dangling("sweep_step");
        if self.phase != GcPhase::Sweep {
            #[cfg(any(debug_assertions, feature = "gc-verify"))]
            self.verify_slow_bits("sweep_step");
            true
        } else {
            false
        }
    }

    /// Sweep up to `budget` objects from after `sweep_prev` (with
    /// `to_live`, until one is kept); at the end of the list the sweep is
    /// over. Returns how many were swept.
    pub(super) fn sweep_objects(&mut self, budget: usize, to_live: bool) -> usize {
        let mut n = 0;
        let new_white = self.current_white;
        // SAFETY: `sweep_prev`, when not null, is an object on `all` kept by
        // this sweep, so allocated; the link read through it (or `all`) heads
        // the part not swept yet, whose objects this heap allocated and has
        // not freed; a dead object is unlinked before `free_obj` and not
        // touched after it
        unsafe {
            while n < budget {
                let link: *mut *mut GcHeader = if self.sweep_prev.is_null() {
                    ptr::addr_of_mut!(self.all)
                } else {
                    ptr::addr_of_mut!((*self.sweep_prev).next)
                };
                let cur = *link;
                if cur.is_null() {
                    self.end_sweep();
                    break;
                }
                n += 1;
                let f = (*cur).flags;
                if is_white(f) && (f & new_white) == 0 {
                    *link = (*cur).next;
                    if to_live && self.in_string_sweep(cur) {
                        // PUC's string table holds it, not the list it
                        // sweeps to the first live object: its string
                        // sweep comes first
                        self.strt.held.push(cur);
                        continue;
                    }
                    self.free_obj(cur);
                    self.live -= 1;
                } else {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    self.sweep_prev = cur;
                    if to_live {
                        break;
                    }
                }
            }
        }
        n
    }

    /// PUC `sweeplist` of `quota` objects of PUC's: sweep in place until
    /// that many of PUC's objects are swept or the list ends. luna's own
    /// objects are swept without counting; so are the ones PUC keeps on
    /// other lists (with a finalizer; 5.1's strings and 5.2's short ones,
    /// in the string table); a record of PUC's objects counts each one, and
    /// a dead one may be freed in part. Returns how many were swept and
    /// whether the list ended.
    pub(crate) fn sweep_counted(&mut self, quota: i64) -> (i64, bool) {
        let mut n = 0;
        let new_white = self.current_white;
        let strt = matches!(self.table_dialect, Dialect::L51 | Dialect::L52);
        // SAFETY: as in `sweep_objects`
        unsafe {
            loop {
                let link: *mut *mut GcHeader = if self.sweep_prev.is_null() {
                    ptr::addr_of_mut!(self.all)
                } else {
                    ptr::addr_of_mut!((*self.sweep_prev).next)
                };
                let cur = *link;
                if cur.is_null() {
                    self.end_sweep();
                    #[cfg(feature = "gc-verify")]
                    self.verify_no_dangling("sweep_counted");
                    return (n, true);
                }
                if n >= quota {
                    #[cfg(feature = "gc-verify")]
                    self.verify_no_dangling("sweep_counted");
                    return (n, false);
                }
                let f = (*cur).flags;
                let dead = is_white(f) && (f & new_white) == 0;
                if (*cur).tag == ObjTag::Ghost {
                    if dead {
                        let left = self.free_ghost_records(cur as *mut Ghost, quota - n);
                        n += left.0;
                        if left.1 {
                            *link = (*cur).next;
                            self.free_block(cur as *mut Ghost);
                            self.live -= 1;
                        }
                        continue;
                    }
                    n += (*(cur as *mut Ghost)).sizes.len() as i64;
                } else if self.puc_listed(cur, strt) {
                    n += 1;
                }
                if dead {
                    *link = (*cur).next;
                    self.free_obj(cur);
                    self.live -= 1;
                } else {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    self.sweep_prev = cur;
                }
            }
        }
    }

    /// Whether PUC keeps the object behind `h` on the list it sweeps
    /// (`allgc`, 5.1 `rootgc`): one of PUC's, with no finalizer pending,
    /// and (`strt`) no string the string table holds.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    unsafe fn puc_listed(&self, h: *mut GcHeader, strt: bool) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            if is_uncounted(h) || (*h).flags & FIN != 0 {
                return false;
            }
            match (*h).tag {
                ObjTag::Str => !strt || (self.table_dialect == Dialect::L52 && !(*(h as *const LuaStr)).is_short()),
                ObjTag::Table => !self.is_own_table(h as *const Table),
                _ => true,
            }
        }
    }

    /// The sweep reached the end of the list.
    fn end_sweep(&mut self) {
        self.sweep_prev = ptr::null_mut();
        self.phase = GcPhase::Pause;
        self.mem_ctx().gc_cycle_ended(self.bytes);
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
                    if self.table_pool.len() < TABLE_POOL_CAP && self.table_pool.try_reserve(1).is_ok() {
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
                        #[cfg(feature = "gc-trace")]
                        if self.trace.where_ {
                            let b = String::from_utf8_lossy(string::bytes_of(s)).into_owned();
                            self.trace.line(format_args!("s {b:?}"));
                        }
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

impl Heap {
    /// The objects of PUC's that `h`, an uncounted object of luna's, stood
    /// for (`PucState::stand_ins`) are freed with it.
    fn free_stand_in(&mut self, h: *mut GcHeader) {
        if let Some(i) = self.puc.stand_ins.iter().position(|(p, _)| *p == h) {
            let (_, sizes) = self.puc.stand_ins.swap_remove(i);
            for s in sizes {
                self.acct_free(s);
            }
        }
    }
}
