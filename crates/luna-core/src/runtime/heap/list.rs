//! The object list and the collector's colour and phase, as the PUC model
//! (`runtime::puc`) reads and changes them: its records live on this list.

use super::*;

/// In the `next` of an object kept off the list: PUC has not made it yet.
const DETACHED: *mut GcHeader = ptr::without_provenance_mut(1);

/// In the `next` of an open upvalue kept off the list (5.1, 5.2): on its
/// thread, where PUC keeps it until it closes.
pub(super) const ON_THREAD: *mut GcHeader = ptr::without_provenance_mut(2);

impl Heap {
    /// A new object `h` kept off the list until PUC makes it ([`Heap::link_now`]
    /// then puts it on), white as a new object for the barriers that reach
    /// it first.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap on no list.
    #[inline]
    pub(in crate::runtime) unsafe fn detach_new(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            (*h).next = DETACHED;
            (*h).paint(self.white());
        }
    }

    /// Whether `h` is kept off the list ([`Heap::detach_new`]).
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    #[inline]
    pub(in crate::runtime) unsafe fn is_detached(h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        unsafe { (*h).next == DETACHED }
    }

    /// The white of objects born now (PUC `luaC_white(g)`).
    #[inline]
    pub(in crate::runtime) fn white(&self) -> u8 {
        self.current_white
    }

    /// The phase of the cycle under way.
    #[inline]
    pub(in crate::runtime) fn phase(&self) -> GcPhase {
        self.phase
    }

    /// The newest object on the list (`allgc`).
    #[inline]
    pub(in crate::runtime) fn list_head(&self) -> *mut GcHeader {
        self.all
    }

    /// The heap is being dropped: nothing is counted.
    #[inline]
    pub(in crate::runtime) fn is_dropping(&self) -> bool {
        self.dropping
    }

    /// The threads the collector visits.
    #[inline]
    pub(in crate::runtime) fn threads(&self) -> &[Gc<crate::runtime::Coro>] {
        &self.threads
    }

    /// Each interned string.
    pub(in crate::runtime) fn each_string(&self, f: impl FnMut(Gc<LuaStr>)) {
        self.strings.each(f);
    }

    /// [`Heap::link`] onto the list.
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[inline]
    pub(in crate::runtime) unsafe fn link_now(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; `udata_after` is the main thread,
        // which lives as long as the heap
        unsafe {
            // born the current white in every phase (PUC `luaC_white(g)`):
            // one made while marking is marked by the atomic step if it is
            // reachable then (the roots are marked again there, and a black
            // object that took it was sent back to gray by its barrier)
            (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.current_white);
            match self.udata_after {
                Some(a) if (*h).tag == ObjTag::Userdata => {
                    let a = a.as_ptr();
                    (*h).next = (*a).next;
                    (*a).next = h;
                }
                _ => {
                    (*h).next = self.all;
                    self.all = h;
                }
            }
        }
        self.live += 1;
        self.hook_reborn(h);
    }

    /// The main thread `t` made just now, running: in 5.1 it is the list's
    /// oldest object (PUC's state makes it first), and new userdata go
    /// after it.
    pub(crate) fn main_thread_made(&mut self, t: Gc<crate::runtime::Coro>) {
        // SAFETY: `t` was made just now and nothing else holds it yet
        unsafe { t.as_mut() }.status = crate::runtime::coroutine::CoroStatus::Running;
        if !self.puc_rules().udata_after_main_thread {
            return;
        }
        let h = t.header();
        // SAFETY: the list holds allocated objects; `h` heads it, made last
        unsafe {
            debug_assert!(std::ptr::eq(self.all, h));
            if (*h).next.is_null() {
                return self.udata_after = Some(t.hdr());
            }
            self.all = (*h).next;
            let mut last = self.all;
            while !(*last).next.is_null() {
                last = (*last).next;
            }
            (*last).next = h;
            (*h).next = std::ptr::null_mut();
        }
        self.udata_after = Some(t.hdr());
    }

    /// 5.1: the main thread, on the list as PUC's is.
    #[inline]
    pub(in crate::runtime) fn main_thread_51(&self) -> Option<Gc<GcHeader>> {
        self.udata_after
    }

    /// The open upvalue `uv` was closed: one kept off the list (5.1, 5.2)
    /// goes on its head, as PUC links it (`luaC_linkupval`,
    /// `luaC_checkupvalcolor`): a marked one stays black while marking
    /// (the barrier on its value follows), and turns the current white
    /// after it.
    #[inline(always)]
    pub(crate) fn upval_closed(&mut self, uv: Gc<crate::runtime::function::Upvalue>) {
        let h = uv.header();
        // SAFETY: `uv` is a handle, so its upvalue is allocated; only its
        // header is read and written, and one off the list goes on it once
        unsafe {
            if (*h).next != ON_THREAD {
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

    /// A new open upvalue that no open list took: on the list, for the
    /// sweep to free.
    #[cold]
    pub(crate) fn upval_unlisted(&mut self, uv: Gc<crate::runtime::function::Upvalue>) {
        self.upval_closed(uv);
    }

    /// `h`, off the list, back at its head (a string the string sweep
    /// held and keeps).
    ///
    /// # Safety
    /// `h` heads a live object of this heap on no list.
    pub(in crate::runtime) unsafe fn relink(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe { (*h).next = self.all };
        self.all = h;
    }

    /// `h`, off the list, freed (a string the string sweep held and frees).
    ///
    /// # Safety
    /// `h` heads a dead object of this heap on no list.
    pub(in crate::runtime) unsafe fn free_unlisted(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe { self.free_obj(h) };
        self.live -= 1;
    }

    /// Move the object `h` on the list to just before `anchor`; nothing
    /// when `h` is `anchor`, is not on the list or the sweep stands at it.
    ///
    /// # Safety
    /// `h` and `anchor` head live objects of this heap.
    pub(in crate::runtime) unsafe fn move_before(
        &mut self,
        h: *mut GcHeader,
        anchor: *mut GcHeader,
    ) {
        if h == anchor || h == self.sweep_prev {
            return;
        }
        // SAFETY: the caller's contract; the links walked are the list's
        unsafe {
            let mut link: *mut *mut GcHeader = ptr::addr_of_mut!(self.all);
            while !(*link).is_null() && *link != h {
                link = ptr::addr_of_mut!((**link).next);
            }
            if (*link).is_null() {
                return;
            }
            *link = (*h).next;
            let mut at: *mut *mut GcHeader = ptr::addr_of_mut!(self.all);
            while !(*at).is_null() && *at != anchor {
                at = ptr::addr_of_mut!((**at).next);
            }
            if (*at).is_null() {
                // not found after all: back at the head
                (*h).next = self.all;
                self.all = h;
                return;
            }
            (*h).next = anchor;
            *at = h;
        }
    }
}
