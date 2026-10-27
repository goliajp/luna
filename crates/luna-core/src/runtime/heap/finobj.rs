//! The objects with finalizers on lists of their own, as PUC keeps them:
//! from 5.2 on an object leaves `all` for `finobj` when it gets a
//! finalizer (`luaC_checkfinalizer`); 5.1's userdata stay after the main
//! thread. The atomic step moves the unreachable ones to the end of
//! `tobefnz` (`separatetobefnz`, 5.1 `luaC_separateudata`), and each goes
//! back when its finalizer is taken (`udata2finalize`: the head of `all`;
//! 5.1's `GCTM`: after the main thread).

use super::*;

impl Heap {
    /// `h`, on `all`, gets a finalizer. From 5.2 on it moves to `finobj`,
    /// found from the head of `all` as PUC finds it; a sweep standing on it
    /// first sweeps on to the next object it keeps (`sweeptolive`).
    ///
    /// # Safety
    /// `h` heads an allocated table or userdata on `all`.
    pub(super) unsafe fn move_to_finobj(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; the links of `all` lead to
        // allocated objects, `h` among them
        unsafe {
            (*h).flags |= FIN;
            // one a load's reader made is set aside: it goes on `finobj`
            // when it is born (`Heap::link_born`)
            if !self.puc_rules().finobj || Heap::is_detached(h) {
                return;
            }
            if self.phase == GcPhase::Sweep && self.sweep_prev == h {
                let _ = self.sweep_objects(usize::MAX, true);
            }
            // marking keeps its color (`keepinvariant`); a sweep would have
            // turned it white
            if self.phase != GcPhase::Propagate {
                (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.current_white);
            }
            let mut link: *mut *mut GcHeader = ptr::addr_of_mut!(self.all);
            while *link != h {
                link = ptr::addr_of_mut!((**link).next);
            }
            *link = (*h).next;
            self.passed(h, (*h).next);
            (*h).next = self.finobj;
            self.finobj = h;
            self.finobj_len += 1;
        }
    }

    /// `h`, set aside, on the list now: on `finobj` when it was given a
    /// finalizer meanwhile.
    ///
    /// # Safety
    /// `h` heads a live object of this heap on no list.
    pub(in crate::runtime) unsafe fn link_born(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; `link_now` puts `h` on `all`
        unsafe {
            self.link_now(h);
            if (*h).flags & FIN != 0 {
                (*h).flags &= !FIN;
                self.move_to_finobj(h);
            }
        }
    }

    /// The objects with finalizers found unreachable (`all`: every one, as
    /// the state closes), to the end of `tobefnz` in the order of their
    /// list, each marked through `m` when given (`markbeingfnz`). Returns
    /// PUC's size of what moved (5.1 `luaC_separateudata`'s).
    pub(super) fn separate(&mut self, mut m: Option<&mut Marker>, all: bool) -> i64 {
        let finobj = self.puc_rules().finobj;
        // the object whose `next` is `link` (null: the head of `finobj`)
        let mut prev = ptr::null_mut();
        let mut link: *mut *mut GcHeader = ptr::addr_of_mut!(self.finobj);
        let mut dead = 0;
        // SAFETY: the lists link allocated objects, and 5.1's main thread
        // lives as long as the heap; only headers and links are written
        unsafe {
            if !finobj {
                let Some(main) = self.udata_after else {
                    return 0;
                };
                prev = main.as_ptr();
                link = ptr::addr_of_mut!((*prev).next);
            }
            while !(*link).is_null() {
                let h = *link;
                let f = (*h).flags;
                if f & FIN == 0 || !(all || is_white(f)) {
                    (prev, link) = (h, ptr::addr_of_mut!((*h).next));
                    continue;
                }
                // 5.1: a userdata whose metatable has no `__gc` now is not
                // finalized, and stays
                if !finobj && !self.has_gc_51(h) {
                    (*h).flags = (f & !FIN) | FINALIZED;
                    (prev, link) = (h, ptr::addr_of_mut!((*h).next));
                    continue;
                }
                (*h).flags = f | FINALIZED;
                dead += self.puc_objsize(h) as i64;
                *link = (*h).next;
                if finobj {
                    self.finobj_len -= 1;
                } else {
                    self.passed(h, (*h).next);
                    if self.sweep_prev == h {
                        self.sweep_prev = prev;
                    }
                }
                (*h).next = ptr::null_mut();
                match self.tobefnz_end.is_null() {
                    true => self.tobefnz = h,
                    false => (*self.tobefnz_end).next = h,
                }
                self.tobefnz_end = h;
                self.tobefnz_len += 1;
                if let Some(m) = m.as_deref_mut() {
                    m.header(h);
                }
            }
        }
        dead
    }

    /// 5.1: whether the metatable of `h` has a `__gc` (`fasttm`).
    ///
    /// # Safety
    /// `h` heads an allocated object.
    unsafe fn has_gc_51(&self, h: *mut GcHeader) -> bool {
        let Some(name) = self.gc_event_51 else {
            return true;
        };
        // SAFETY: the caller's contract; a userdata's tag says what it heads
        unsafe {
            (*h).tag != ObjTag::Userdata
                || (*(h as *const crate::runtime::Userdata))
                    .metatable()
                    .is_some_and(|mt| !mt.get_str(name).is_nil())
        }
    }

    /// The first object awaiting its finalizer, off `tobefnz` and back
    /// where PUC puts it.
    pub(super) fn tobefnz_pop(&mut self) -> Option<*mut GcHeader> {
        let h = self.tobefnz;
        if h.is_null() {
            return None;
        }
        // SAFETY: `tobefnz` links allocated objects, and so is the main
        // thread; only headers and links are written
        unsafe {
            self.tobefnz = (*h).next;
            if self.tobefnz.is_null() {
                self.tobefnz_end = ptr::null_mut();
            }
            self.tobefnz_len -= 1;
            let f = (*h).flags & !FIN;
            // marking keeps its color (`keepinvariant`)
            (*h).flags = match self.phase {
                GcPhase::Propagate => f,
                _ => (f & !(COLOR_BITS | SLOW)) | self.current_white | (((*h).cls & RO_CLS) << 1),
            };
            if let Some(a) = self.udata_after {
                let a = a.as_ptr();
                (*h).next = (*a).next;
                (*a).next = h;
                return Some(h);
            }
            let head = self.all;
            (*h).next = head;
            self.all = h;
            // no object made since the last safe point: still none
            if (self.young == head) | (self.rooted_young == head) {
                self.passed_to(head, h);
            }
        }
        Some(h)
    }

    /// A sweep ended: the objects with finalizers, which the marking
    /// kept black, go back to white as PUC's sweep of their lists turns
    /// them, so that the next marking traverses them again.
    pub(super) fn whiten_fin_lists(&mut self) {
        let white = self.current_white;
        for mut cur in [self.finobj, self.tobefnz] {
            // SAFETY: the lists link allocated objects; only headers are written
            unsafe {
                while !cur.is_null() {
                    let (f, cls) = ((*cur).flags, (*cur).cls);
                    (*cur).flags = (f & !(COLOR_BITS | SLOW)) | white | ((cls & RO_CLS) << 1);
                    cur = (*cur).next;
                }
            }
        }
    }

    /// `h` left `all`, `next` after it: the boundaries of the young
    /// objects at it move on.
    fn passed(&mut self, h: *mut GcHeader, next: *mut GcHeader) {
        if (self.young == h) | (self.rooted_young == h) {
            self.passed_to(h, next);
        }
    }

    #[cold]
    fn passed_to(&mut self, from: *mut GcHeader, to: *mut GcHeader) {
        for b in [&mut self.young, &mut self.rooted_young] {
            if *b == from {
                *b = to;
            }
        }
    }

    /// The heads of the lists whose objects a marking colors.
    pub(super) fn marked_lists(&self) -> [*mut GcHeader; 3] {
        [self.all, self.finobj, self.tobefnz]
    }

    /// The heads of every list of objects, `tobefnz` last.
    pub(super) fn object_lists(&self) -> [*mut GcHeader; 4] {
        [self.all, self.fixed, self.finobj, self.tobefnz]
    }
}
