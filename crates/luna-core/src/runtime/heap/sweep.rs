//! Sweeping and freeing objects.

use super::*;

impl Heap {
    /// Sweep the whole `all` list in one pass: free dead-white objects, survivors to the new
    /// current-white so the next cycle can re-mark them. Returns the number of objects freed.
    pub(super) fn full_sweep(&mut self) -> usize {
        self.sweep_begins();
        // detach the list first so freeing never aliases a pointer into self
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
                self.puc_sweep_reach(cur);
                let f = (*cur).flags;
                // dead = other-white (i.e. white but not current-white).
                // Survivors are BLACK (just-marked) or current-white (born
                // during the sweep itself).
                let fate = if is_white(f) && (f & new_white) == 0 {
                    self.puc_sweep_dead(cur, false)
                } else {
                    Fate::Keep
                };
                if let Fate::Free(_) = fate {
                    *link = (*cur).next;
                    self.free_obj(cur);
                    freed += 1;
                } else {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    link = ptr::addr_of_mut!((*cur).next);
                    self.puc_sweep_kept(cur);
                }
            }
            self.all = head;
        }
        self.puc_sweep_ends();
        self.live -= freed;
        // a sweep keeps the order of `all` (see `Heap::keep_young`)
        self.check_no_dangling("full_sweep");
        self.check_slow_bits("full_sweep");
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
        // after every step: the mutator runs between them
        self.check_no_dangling("sweep_step");
        if self.phase != GcPhase::Sweep {
            self.check_slow_bits("sweep_step");
            true
        } else {
            false
        }
    }

    /// Sweep up to `budget` objects after `sweep_prev` (`to_live`: to the first kept); returns the count.
    pub(super) fn sweep_objects(&mut self, budget: usize, to_live: bool) -> usize {
        self.sweep_begins();
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
                self.puc_sweep_reach(cur);
                let f = (*cur).flags;
                let fate = if is_white(f) && (f & new_white) == 0 {
                    self.puc_sweep_dead(cur, to_live)
                } else {
                    Fate::Keep
                };
                match fate {
                    // PUC's string table holds it, not the list it sweeps to
                    // the first live object: its string sweep comes first
                    Fate::Hold => *link = (*cur).next,
                    Fate::Free(_) => {
                        *link = (*cur).next;
                        self.free_obj(cur);
                        self.live -= 1;
                    }
                    Fate::Keep => {
                        (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                        self.sweep_prev = cur;
                        self.puc_sweep_kept(cur);
                        if to_live {
                            break;
                        }
                    }
                }
            }
        }
        n
    }

    /// PUC `sweeplist` of `quota` objects of PUC's, in place, or to the end
    /// of the list. luna's own objects and the ones PUC keeps on other lists
    /// (with a finalizer; 5.1's strings and 5.2's short ones) are swept
    /// uncounted; a record of PUC's objects counts each one, and a dead one
    /// may be freed in part. Returns the count and whether the list ended.
    pub(crate) fn sweep_counted(&mut self, quota: i64) -> (i64, bool) {
        self.sweep_begins();
        let mut n = 0;
        let new_white = self.current_white;
        let strt = self.puc_rules().sweeps_string_table;
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
                    self.check_no_dangling("sweep_counted");
                    return (n, true);
                }
                if n >= quota && self.puc_has_more(cur) {
                    self.check_no_dangling("sweep_counted");
                    return (n, false);
                }
                n += self.puc_sweep_reach(cur);
                let (f, cls) = ((*cur).flags, (*cur).cls);
                let dead = is_white(f) && (f & new_white) == 0;
                self.hook_swept(cur, dead, strt);
                if self.puc_counts_in_sweep(cur) {
                    n += 1;
                } else if (*cur).tag == ObjTag::Ghost {
                    match self.puc_sweep_record(cur, dead, quota - n) {
                        (k, Some(gone)) => {
                            n += k;
                            if gone {
                                *link = (*cur).next;
                                self.free_block(cur as *mut Ghost);
                                self.live -= 1;
                            }
                            continue;
                        }
                        (k, None) => n += k,
                    }
                }
                let fate = if dead {
                    self.puc_sweep_dead(cur, false)
                } else {
                    Fate::Keep
                };
                if let Fate::Free(k) = fate {
                    n += k;
                    *link = (*cur).next;
                    // gc-verify notes every free in `free_obj`; 5.1 and 5.2
                    // strings mostly go through the string table's sweep
                    if !crate::runtime::gc_hooks::VERIFY && !strt && Self::is_plain_str(cur) {
                        self.free_plain_str(cur as *mut LuaStr);
                    } else {
                        self.free_obj(cur);
                    }
                    self.live -= 1;
                } else {
                    // white, so SLOW only for a read-only table (`with_slow`)
                    (*cur).flags = (f & !(COLOR_BITS | SLOW)) | new_white | ((cls & RO_CLS) << 1);
                    self.sweep_prev = cur;
                    n += self.puc_sweep_kept(cur);
                }
            }
        }
    }

    /// Whether an object PUC has is at `cur` or after it: luna's own
    /// objects at the list's end take no step of PUC's, whose list has
    /// ended.
    ///
    /// # Safety
    /// `cur` is null or heads an allocated object of this heap.
    unsafe fn puc_has_more(&self, mut cur: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract; the list links allocated objects
        unsafe {
            while !cur.is_null() {
                if self.puc_counts_in_sweep(cur) || (*cur).tag == ObjTag::Ghost {
                    return true;
                }
                cur = (*cur).next;
            }
        }
        false
    }

    /// The sweep reached the end of the list (once a cycle: kept out of the
    /// sweep's loop).
    #[cold]
    #[inline(never)]
    fn end_sweep(&mut self) {
        self.puc_sweep_ends();
        self.sweep_prev = ptr::null_mut();
        self.phase = GcPhase::Pause;
        self.mem_ctx().gc_cycle_ended(self.puc_bytes());
    }
}
