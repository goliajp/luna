//! Upvalues kept off the object list. 5.1 and 5.2 keep an open upvalue on
//! its thread and put it on the list as it closes. 5.3's upvalues are no
//! collectable objects at all: each counts the closures that hold it, and
//! goes when the last of them is freed, or as it closes with none holding
//! it (`luaC_upvdeccount`, `luaF_close`).

use super::*;
use crate::runtime::function::{UpvalState, Upvalue};

/// In the `next` of an open upvalue kept on its thread (5.1, 5.2), where
/// PUC keeps it until it closes.
pub(in crate::runtime) const ON_THREAD: *mut GcHeader = ptr::without_provenance_mut(usize::MAX);

/// In the `next` of a 5.3 upvalue, which is never on the list, never
/// marked, and black for good: a barrier on it marks the value it takes
/// while the invariant holds, as PUC's `luaC_upvalbarrier` does.
pub(in crate::runtime) const HELD: *mut GcHeader = ptr::without_provenance_mut(usize::MAX - 1);

/// In a 5.3 upvalue's per-type byte: a closure the marking traversed holds
/// it (PUC's `u.open.touched`).
pub(in crate::runtime) const TOUCHED: u8 = 1;

/// Whether `next` marks an upvalue kept off the list.
#[inline(always)]
fn off_list(next: *mut GcHeader) -> bool {
    next.addr() >= HELD.addr()
}

impl Heap {
    /// The open upvalue `uv` was closed: one kept on its thread (5.1, 5.2)
    /// goes on the list's head, as PUC links it (`luaC_linkupval`,
    /// `luaC_checkupvalcolor`): a marked one stays black while marking
    /// (the barrier on its value came before), and turns the current
    /// white after it. A 5.3 one no closure holds goes (`luaF_close`).
    #[inline(always)]
    pub(crate) fn upval_closed(&mut self, uv: Gc<Upvalue>) {
        let h = uv.header();
        // SAFETY: `uv` is a handle, so its upvalue is allocated; only its
        // header is read here
        if off_list(unsafe { (*h).next }) {
            // SAFETY: as above
            unsafe { self.closed_off_list(h) };
        }
    }

    /// [`Heap::upval_closed`] of one off the list.
    ///
    /// # Safety
    /// `h` heads an allocated upvalue of this heap off the list.
    unsafe fn closed_off_list(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; a 5.3 one no closure holds is held
        // by nothing else once off its thread
        unsafe {
            if (*h).next == HELD {
                if (*h).aux == 0 {
                    self.free_obj(h);
                }
                return;
            }
            (*h).next = self.all;
            if self.phase != GcPhase::Propagate && !is_white((*h).flags()) {
                (*h).paint(self.current_white);
            }
        }
        self.all = h;
        self.live += 1;
    }

    /// A new open upvalue that no open list took and nothing holds:
    /// closed, on the list for the sweep to free, or (5.3) freed.
    #[cold]
    pub(crate) fn upval_unlisted(&mut self, uv: Gc<Upvalue>) {
        // SAFETY: `uv` is a handle, so its upvalue is allocated; nothing
        // borrows it, and no thread lists it to close it later
        unsafe { uv.as_mut() }.set_closed(Value::Nil);
        self.upval_closed(uv);
    }

    /// A closure took `uv` (PUC 5.3's `refcount++`; counted in every
    /// dialect, read in 5.3 alone).
    #[inline(always)]
    pub(crate) fn upval_held(&mut self, uv: Gc<Upvalue>) {
        let h = uv.header();
        // SAFETY: `uv` is a handle, so its upvalue is allocated; only its
        // per-type word is written
        unsafe { (*h).aux = (*h).aux.wrapping_add(1) };
    }

    /// A closure let `uv` go, freed or given another: a 5.3 one closed and
    /// held by no closure now goes (`luaC_upvdeccount`).
    ///
    /// # Safety
    /// `uv` heads an allocated upvalue of this heap (one a dead closure
    /// held may be freed already, where upvalues are objects: called for
    /// those in 5.3 alone).
    #[inline(always)]
    pub(crate) unsafe fn upval_let_go(&mut self, uv: *mut Upvalue) {
        let h = uv as *mut GcHeader;
        // SAFETY: the caller's contract
        unsafe {
            let n = (*h).aux.wrapping_sub(1);
            (*h).aux = n;
            if n == 0 && (*h).next == HELD && matches!((*uv).state, UpvalState::Closed(_)) {
                self.free_obj(h);
            }
        }
    }

    /// The new closure `nc` (made to be filled) takes `uv` in place `i`.
    #[inline(always)]
    pub(crate) fn put_upval(
        &mut self,
        nc: Gc<crate::runtime::function::LuaClosure>,
        i: usize,
        uv: Gc<Upvalue>,
    ) {
        // SAFETY: `nc` is a handle, so its closure is allocated; made just
        // now, only its maker holds it, and the borrow covers one store; it
        // is white (a collection meanwhile ran to its pause at most), so
        // the store needs no barrier
        unsafe { nc.as_mut() }.upvals_mut()[i] = uv;
        self.upval_held(uv);
    }

    /// Closure `cl`'s upvalue `i` replaced by `uv`, with the barrier a
    /// black closure needs.
    pub(crate) fn set_closure_upval(
        &mut self,
        cl: Gc<crate::runtime::function::LuaClosure>,
        i: usize,
        uv: Gc<Upvalue>,
    ) {
        self.upval_held(uv);
        // SAFETY: `cl` is a handle, so its closure is allocated; no other
        // reference into it is live, and the borrow covers one swap; the
        // old upvalue is allocated while `cl` held it
        unsafe {
            let old = std::mem::replace(&mut cl.as_mut().upvals_mut()[i], uv);
            self.upval_let_go(old.as_ptr());
        }
        self.barrier_back(cl);
    }

    /// Thread `co` is freed (PUC `luaE_freethread`, `luaF_close`): its
    /// open upvalues close with their values, as when the thread closes
    /// them (one no 5.3 closure holds goes).
    ///
    /// # Safety
    /// `co` heads an allocated thread of this heap, freed next.
    pub(super) unsafe fn upvals_of_freed_thread(&mut self, co: *mut crate::runtime::Coro) {
        // SAFETY: the caller's contract; each listed upvalue is allocated,
        // and nothing borrows it
        unsafe {
            let co = &mut *co;
            let list = co.open_upvals.take();
            let r = self.puc_rules();
            // the heap is dropped: an upvalue on the list may be freed
            // already, and goes anyway
            if self.dropping && !(r.open_upvals_on_threads || r.upvals_refcounted) {
                return;
            }
            for &(slot, uv) in list.iter() {
                // and one kept on the thread goes now, as the list it would
                // go on is being freed
                if self.dropping && (*uv.header()).next == ON_THREAD {
                    self.free_obj(uv.header());
                    continue;
                }
                let v = co.stack.get(slot as usize).copied().unwrap_or(Value::Nil);
                (*uv.as_ptr()).set_closed(v);
                self.upval_closed(uv);
            }
        }
    }

    /// The atomic step (PUC `remarkupvals`): a thread no marking reached
    /// may still have open upvalues a live closure reached, whose slots may
    /// have taken new values since; those values are marked.
    pub(crate) fn remark_open_upvals(&self, m: &mut Marker) {
        let held = self.puc_rules().upvals_refcounted;
        for &co in self.threads() {
            if !is_white(co.hdr().flags()) {
                continue;
            }
            for &(slot, uv) in co.open_upvals.iter() {
                let h = uv.header();
                // SAFETY: a listed upvalue is allocated; only its header is
                // read and its per-type byte written
                let reached = unsafe {
                    if held {
                        let t = (*h).sub & TOUCHED != 0;
                        (*h).sub &= !TOUCHED;
                        t
                    } else {
                        !is_white((*h).flags())
                    }
                };
                if reached && let Some(&v) = co.stack.get(slot as usize) {
                    m.value(v);
                }
            }
        }
    }
}
