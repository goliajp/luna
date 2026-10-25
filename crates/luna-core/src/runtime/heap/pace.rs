//! Byte accounting and collection pacing.

use super::*;

impl Heap {
    /// Number of GC-managed objects currently linked into the heap (live + not
    /// yet swept). Useful for `collectgarbage("count")`-style introspection.
    pub fn live_objects(&self) -> usize {
        self.live
    }

    /// Pure-read walk over the intrusive `all`
    /// objects list, invoking `visit(tag)` once per live (or
    /// not-yet-swept) GC-managed object. Used by `luna-tools`'s
    /// `luna-heap-dump` to build a per-type histogram; embedders
    /// can reuse it for ad-hoc heap introspection.
    ///
    /// # Read-only contract
    ///
    /// The callback receives only the [`ObjTag`] discriminant and
    /// is invoked under a `&self` borrow on the heap: no pointer
    /// to the GC payload escapes, no `as_mut`-style aliasing is
    /// available, and the walk performs zero allocation in the
    /// loop. Safe to call between dispatch ticks (the only allocs
    /// happen in the caller's bookkeeping).
    ///
    /// The walk visits the `all` list (in a sweep, with the dead objects
    /// not yet swept), the never-swept fixed list and the parked objects
    /// due for their finalizers, so a mid-cycle
    /// invocation reports the same total as [`Heap::live_objects`].
    pub fn walk_objects(&self, mut visit: impl FnMut(ObjTag)) {
        for head in [self.all, self.fixed, self.parked] {
            let mut cur = head;
            while !cur.is_null() {
                // SAFETY: pointers come from the runtime's
                // intrusive all-objects list. `&self` borrow on
                // the heap prevents concurrent mutation; the GC
                // cannot run while this walk holds the borrow,
                // so every `next` link is valid until consumed.
                let (tag, next) = unsafe { ((*cur).tag, (*cur).next) };
                visit(tag);
                cur = next;
            }
        }
    }

    /// Every live object is rooted now (a collection check): objects
    /// allocated from here on are young until the next such point. The
    /// newest one so far may be held only by a native that just made it
    /// (a check after an API call), unless the check is one where the
    /// stack holds every live value ([`Heap::rooted_safe_point`]).
    #[inline(always)]
    pub(crate) fn safe_point(&mut self) {
        self.young = self.all;
    }

    /// A sweep begins: it may free the object the last safe point left as
    /// the boundary, and its address be reused. A rooted boundary is
    /// forgotten; one the sweep frees too, every object counting as young
    /// until the next safe point (as after the atomic step).
    #[inline(always)]
    pub(crate) fn sweep_begins(&mut self) {
        self.rooted_young = ptr::null_mut();
        let b = self.young;
        // SAFETY: the boundary is a live object: every sweep, the only
        // thing that frees one, forgets it first when it is dead
        if !b.is_null() && unsafe { is_white((*b).flags) && (*b).flags & self.current_white == 0 } {
            self.young = ptr::null_mut();
        }
    }

    /// The next safe point is one where the stack holds every live value.
    #[inline(always)]
    pub(crate) fn rooted_safe_point(&mut self) {
        self.rooted_young = self.all;
    }

    /// How many objects were allocated since the last
    /// [`Heap::safe_point`]: the ones on `all` before the boundary. A sweep
    /// keeps them (they are the current white) and their order, and may
    /// free the boundary itself, so an emergency collection counts them
    /// before it sweeps anything. `None` while every object counts as
    /// young (no safe point since the state was made or since an atomic
    /// step), dead ones a pending sweep frees among them. With whether a
    /// rooted safe point left the boundary (see [`Heap::rooted_safe_point`]).
    pub(crate) fn young_len(&self) -> (Option<usize>, bool) {
        if self.young.is_null() {
            return (None, false);
        }
        let mut n = 0;
        let mut cur = self.all;
        // SAFETY: the links of `all` lead to allocated objects
        unsafe {
            while !cur.is_null() && cur != self.young {
                n += 1;
                cur = (*cur).next;
            }
        }
        (Some(n), self.young == self.rooted_young)
    }

    /// Mark the young objects (see [`Heap::young_len`]), every object on
    /// `all` for `None`: an emergency collection runs in the middle of an
    /// operation, whose new objects may be held only by Rust locals. The
    /// newest object at the safe point too, unless the stack held it then
    /// (it dies as PUC's would).
    pub(crate) fn mark_young(&self, m: &mut Marker, n: (Option<usize>, bool)) {
        let (n, rooted) = n;
        let mut cur = self.all;
        // SAFETY: the first `n` objects on `all` (all of them for `None`)
        // are allocated (nothing is freed while marking); only the header
        // is touched
        unsafe {
            for _ in 0..n.unwrap_or(usize::MAX) {
                if cur.is_null() {
                    break;
                }
                m.header(cur);
                cur = (*cur).next;
            }
            if !self.young.is_null() && !rooted {
                m.header(self.young);
            }
        }
    }

    /// After a collection that kept the young objects (see
    /// [`Heap::young_len`]): they lead `all` in their order, so the
    /// boundary is the object after them, whatever became of the one
    /// before.
    pub(crate) fn keep_young(&mut self, n: (Option<usize>, bool)) {
        let (n, rooted) = n;
        let Some(n) = n else {
            self.young = ptr::null_mut();
            return;
        };
        let mut cur = self.all;
        // SAFETY: the first `n` objects on `all` are the young ones, kept
        // by the collection; the links lead to live objects
        unsafe {
            for _ in 0..n {
                cur = (*cur).next;
            }
        }
        self.young = cur;
        if rooted {
            self.rooted_young = cur;
        }
    }
}
