//! The objects whose finalizers are due, off the object list as PUC's are
//! on `tobefnz` (5.1 `tmudata`), and where each goes back once its
//! finalizer is taken: the head of `all` (`udata2finalize`), in 5.1 after
//! the main thread (`GCTM`).
//!
//! An object due is `DEFERRED` from the atomic step that found it dead,
//! and stays where it is until a sweep meets it; the sweep parks it
//! (`PARKED`, `DEFERRED` cleared). Its finalizer taken while it is parked,
//! it goes back at once; taken before a sweep met it, it is marked
//! (`DEFERRED` and `PARKED`), and the sweep puts it back when it meets it.

use super::*;

/// In `cls` of a table or userdata whose finalizer is due: parked off
/// `all`, or (with `DEFERRED`) to be put back by the sweep that meets it.
pub(in crate::runtime) const PARKED: u8 = 0x10;

/// What a sweep does with an object due it met.
pub(super) enum Due {
    /// taken off where the sweep is: it goes on with the next one
    Moved,
    /// back to an ordinary object where it is (the head of `all`, where
    /// PUC's sweep meets it as well)
    Here,
}

impl Heap {
    /// A sweep meets `cur`, which is `DEFERRED`, through `link`; `at_head`:
    /// `link` is the head of the list it sweeps.
    ///
    /// # Safety
    /// `cur` is the allocated object `*link` points at.
    #[cold]
    #[inline(never)]
    pub(super) unsafe fn sweep_meets_due(
        &mut self,
        link: *mut *mut GcHeader,
        cur: *mut GcHeader,
        at_head: bool,
    ) -> Due {
        // SAFETY: the caller's contract; only `cur`'s header and the links
        // around it are written
        unsafe {
            let cls = (*cur).cls;
            (*cur).flags &= !DEFERRED;
            let parked = cls & PARKED != 0;
            (*cur).cls = cls ^ PARKED;
            if parked && at_head && self.udata_after.is_none() {
                return Due::Here;
            }
            // off the list: the boundary of the young objects moves on
            if (self.young == cur) | (self.rooted_young == cur) {
                self.young_passed_to(cur, (*cur).next);
            }
            *link = (*cur).next;
            if parked {
                // where the sweep has been: white as it leaves its survivors
                (*cur).flags = white_flags((*cur).flags, cls, self.current_white);
                self.put_back(Gc::from_ptr_unchecked(cur));
            } else {
                // at the end: the finalizers are taken in the order the
                // sweep meets their objects, from the front; its color the
                // marking's until `whiten_parked`
                (*cur).next = ptr::null_mut();
                match self.parked_end.is_null() {
                    true => self.parked = cur,
                    false => (*self.parked_end).next = cur,
                }
                self.parked_end = cur;
            }
            Due::Moved
        }
    }

    /// The finalizer of `g`, an object due just taken off `tobefnz`, is
    /// taken (`udata2finalize`).
    pub(super) fn finalizer_taken(&mut self, g: Gc<GcHeader>) {
        let h = g.as_ptr();
        // SAFETY: a handle's object is allocated, and so are the parked
        // objects linked from `parked`
        unsafe {
            let (f, cls) = ((*h).flags, (*h).cls);
            if f & DEFERRED != 0 {
                (*h).cls = cls | PARKED;
            } else if cls & PARKED != 0 {
                (*h).cls = cls & !PARKED;
                // off the parked list, mostly its front
                let next = (*h).next;
                if self.parked == h {
                    self.parked = next;
                    if next.is_null() {
                        self.parked_end = next;
                    }
                } else {
                    let mut prev = self.parked;
                    while (*prev).next != h {
                        prev = (*prev).next;
                    }
                    (*prev).next = next;
                    if next.is_null() {
                        self.parked_end = prev;
                    }
                }
                // marking keeps its color (`keepinvariant`)
                if self.phase != GcPhase::Propagate {
                    (*h).flags = white_flags((*h).flags, cls, self.current_white);
                }
                self.put_back(g);
            }
        }
    }

    /// A sweep ended: the parked objects, which the marking kept black,
    /// go back to white as the sweep turns every survivor (PUC sweeps
    /// `tobefnz` too, 5.1 whitens `tmudata` as it marks it), so that the
    /// next marking traverses them again.
    pub(super) fn whiten_parked(&mut self) {
        let white = self.current_white;
        let mut cur = self.parked;
        // SAFETY: the parked list links allocated objects; only their
        // headers are written
        unsafe {
            while !cur.is_null() {
                (*cur).flags = white_flags((*cur).flags, (*cur).cls, white);
                cur = (*cur).next;
            }
        }
    }

    /// The young objects' boundaries at `from` move to `to`.
    #[cold]
    fn young_passed_to(&mut self, from: *mut GcHeader, to: *mut GcHeader) {
        for b in [&mut self.young, &mut self.rooted_young] {
            if *b == from {
                *b = to;
            }
        }
    }

    /// `g`, on no list, where PUC puts a finalized object back.
    fn put_back(&mut self, g: Gc<GcHeader>) {
        let h = g.as_ptr();
        // SAFETY: a handle's object is allocated, and so is the main thread
        unsafe {
            if let Some(a) = self.udata_after {
                let a = a.as_ptr();
                (*h).next = (*a).next;
                (*a).next = h;
                return;
            }
            let head = self.all;
            (*h).next = head;
            self.all = h;
            // no object made since the last safe point: still none
            if (self.young == head) | (self.rooted_young == head) {
                self.young_passed_to(head, h);
            }
        }
    }
}

/// The flags of a table or userdata turned white: SLOW only for a
/// read-only table (`GcHeader::with_slow`), its `cls` read already.
#[inline(always)]
fn white_flags(flags: u8, cls: u8, white: u8) -> u8 {
    (flags & !(COLOR_BITS | SLOW)) | white | ((cls & RO_CLS) << 1)
}
