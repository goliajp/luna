//! Sweeping and freeing objects.

use super::*;
use crate::runtime::dialect::Dialect;

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
                self.sweep_open_at(cur);
                self.env51_sweep(cur, false);
                let f = (*cur).flags;
                // dead = other-white (i.e. white but not current-white).
                // Survivors are BLACK (just-marked) or current-white (born
                // during the sweep itself).
                if is_white(f) && (f & new_white) == 0 && !self.kept_dead(cur) {
                    self.env51_freed(cur);
                    *link = (*cur).next;
                    self.free_obj(cur);
                    freed += 1;
                } else {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    link = ptr::addr_of_mut!((*cur).next);
                    self.env51_sweep(cur, true);
                }
            }
            self.all = head;
        }
        self.sweep_open_end();
        self.whiten_unborn();
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
                self.sweep_open_at(cur);
                self.env51_sweep(cur, false);
                let f = (*cur).flags;
                let dead = is_white(f) && (f & new_white) == 0;
                if dead && to_live && self.in_string_sweep(cur) {
                    // PUC's string table holds it, not the list it sweeps to
                    // the first live object: its string sweep comes first
                    *link = (*cur).next;
                    self.puc.strt.held.push(cur);
                } else if dead && !self.kept_dead(cur) {
                    self.env51_freed(cur);
                    *link = (*cur).next;
                    self.free_obj(cur);
                    self.live -= 1;
                } else {
                    (*cur).flags = (*cur).with_slow((f & !COLOR_BITS) | new_white);
                    self.sweep_prev = cur;
                    self.note_swept(cur);
                    self.env51_sweep(cur, true);
                    if to_live {
                        break;
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
        let strt = matches!(self.dialect(), Dialect::L51 | Dialect::L52);
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
                if n >= quota {
                    self.check_no_dangling("sweep_counted");
                    return (n, false);
                }
                self.sweep_open_at(cur);
                n += self.env51_sweep(cur, false);
                let (f, cls) = ((*cur).flags, (*cur).cls);
                let dead = is_white(f) && (f & new_white) == 0;
                self.hook_swept(cur, dead, strt);
                if self.listed(cur) {
                    n += 1;
                } else if (*cur).tag == ObjTag::Ghost {
                    if dead && !self.kept_dead(cur) {
                        let left = self.free_ghost_records(&mut *(cur as *mut Ghost), quota - n);
                        n += left.0;
                        if left.1 {
                            *link = (*cur).next;
                            self.free_block(cur as *mut Ghost);
                            self.live -= 1;
                        }
                        continue;
                    }
                    n += Self::ghost_objects(&*(cur as *mut Ghost));
                }
                if dead && !self.kept_dead(cur) {
                    n += self.env51_sweep(cur, true);
                    self.env51_forget(cur);
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
                    self.note_swept(cur);
                    n += self.env51_sweep(cur, true);
                }
            }
        }
    }

    /// Whether the sweep counts the object behind `h`: one of PUC's on the
    /// list PUC sweeps, not one with a finalizer pending (but in 5.1, whose
    /// userdata stay on it, as does its main thread, and whose library
    /// functions and standard files luna made without counting are PUC's).
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    #[inline]
    pub(super) unsafe fn listed(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            let (f, sub, cls) = ((*h).flags, (*h).sub, (*h).cls);
            if self.dialect() == Dialect::L51 {
                let own = match (*h).tag {
                    ObjTag::Coro => false,
                    ObjTag::Native => cls & LUNA_ONLY != 0,
                    // the standard files, made without counting
                    ObjTag::Userdata => {
                        sub & UNCOUNTED != 0
                            && !matches!(
                                (*(h as *const Userdata)).payload,
                                UserdataPayload::File(_)
                            )
                    }
                    _ => sub & UNCOUNTED != 0,
                };
                // one waiting for its finalizer is on 5.1's `tmudata`
                cls & UNLISTED == 0 && !own && f & DEFERRED == 0
            } else {
                (f & FIN) | (sub & UNCOUNTED) | (cls & UNLISTED) == 0
            }
        }
    }

    /// 5.2's `sweepthread`: a thread the sweep keeps is shrunk, by the VM,
    /// which holds what that needs (`puc.shrunk` lists them for it).
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    #[inline]
    unsafe fn note_swept(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        if self.dialect() == Dialect::L52 && unsafe { (*h).tag } == ObjTag::Coro {
            self.puc.shrunk.push(h);
        }
    }

    /// The sweep reached the end of the list.
    fn end_sweep(&mut self) {
        self.sweep_open_end();
        self.whiten_unborn();
        self.sweep_prev = ptr::null_mut();
        self.phase = GcPhase::Pause;
        self.mem_ctx().gc_cycle_ended(self.puc.bytes);
    }
}
