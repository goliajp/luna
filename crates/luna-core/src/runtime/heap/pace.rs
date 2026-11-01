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
    /// not yet swept), the never-swept fixed list and the objects with
    /// finalizers, so a mid-cycle
    /// invocation reports the same total as [`Heap::live_objects`].
    pub fn walk_objects(&self, mut visit: impl FnMut(ObjTag)) {
        for head in self.object_lists() {
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

    /// A sweep starts: it may free the object the last safe point left as
    /// the boundary, and its address be reused. A boundary the sweep frees
    /// moves to the first object after it the sweep keeps (see
    /// [`Heap::kept_boundary`]); that one is older than the safe point, when
    /// every live object but the newest had a root, so it counts as rooted.
    /// A rooted boundary left behind by later safe points is forgotten, as
    /// the sweep may free it; one that stays the boundary stays rooted.
    pub(crate) fn sweep_starts(&mut self) {
        let b = self.kept_boundary(self.young);
        if b != self.young {
            self.rooted_young = b;
        } else if self.rooted_young != self.young {
            self.rooted_young = ptr::null_mut();
        }
        self.young = b;
    }

    /// A step of a sweep [`Heap::sweep_starts`] started: every boundary set
    /// since is an object the sweep keeps, so this moves nothing unless the
    /// sweep was started otherwise.
    #[inline(always)]
    pub(crate) fn sweep_begins(&mut self) {
        let b = self.kept_boundary(self.young);
        if b != self.young {
            self.boundary_moved(b);
        }
    }

    #[cold]
    fn boundary_moved(&mut self, b: *mut GcHeader) {
        self.rooted_young = b;
        self.young = b;
    }

    /// The boundary `b`, or the first object after it on `all` that the
    /// sweep under way or about to start keeps (null: none). The objects
    /// passed over are dead ones that sweep frees: an object the last safe
    /// point saw is still older than the boundary, and the boundary outlives
    /// the sweep. Called before that sweep frees anything past `b`.
    #[inline(always)]
    pub(crate) fn kept_boundary(&self, mut b: *mut GcHeader) -> *mut GcHeader {
        // SAFETY: the boundary is a live object (every sweep moves it off
        // one it frees, here, before freeing anything past it), and the
        // links of `all` from it lead to allocated objects
        unsafe {
            while !b.is_null() && is_white((*b).flags) && (*b).flags & self.current_white == 0 {
                b = (*b).next;
            }
        }
        b
    }

    /// The next safe point is one where the stack holds every live value.
    #[inline(always)]
    pub(crate) fn rooted_safe_point(&mut self) {
        self.rooted_young = self.all;
    }

    /// How many objects were allocated since the last
    /// [`Heap::safe_point`] and outlive any sweep under way: the ones on
    /// `all` before the boundary, less the dead ones that sweep frees. A
    /// sweep keeps the rest (they are the current white) and their order,
    /// so an emergency collection counts them before it sweeps anything,
    /// and they lead `all` after it. `None` while every object counts as
    /// young (no safe point since the state was made). With whether a
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
                let f = (*cur).flags;
                n += usize::from(!is_white(f) || f & self.current_white != 0);
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
