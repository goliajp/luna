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
                    n += self.ghost_objects(cur as *mut Ghost);
                } else if self.puc_listed(cur, strt) {
                    n += 1;
                }
                if dead {
                    *link = (*cur).next;
                    // gc-verify notes every free in `free_obj`; 5.1 and 5.2
                    // strings mostly go through the string table's sweep
                    if !cfg!(feature = "gc-verify") && !strt && Self::is_plain_str(cur) {
                        self.free_plain_str(cur as *mut LuaStr);
                    } else {
                        self.free_obj(cur);
                    }
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
                ObjTag::Str => {
                    !strt
                        || (self.table_dialect == Dialect::L52
                            && !(*(h as *const LuaStr)).is_short())
                }
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
}
