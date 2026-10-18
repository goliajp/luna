//! The collector left in the middle of a cycle, as PUC's is when an
//! embedder's `luaL_openlibs` ends while a cycle propagates: the objects
//! its record names black or gray coloured so, the gray ones in the order
//! the collector takes them.

use super::*;

impl Heap {
    /// Enter the propagation of a cycle with `black` traversed, `gray` to
    /// be traversed (the first next) and `again` to be traversed in the
    /// atomic step; every other object stays white. What luna's own objects
    /// alone hold below a black one is marked as the cycle ends, uncounted,
    /// as the marking does for it.
    ///
    /// # Safety
    /// Every header heads an allocated object of this heap, and the
    /// collector is in the pause.
    pub(crate) unsafe fn acct_enter_midcycle(
        &mut self,
        black: &[*mut GcHeader],
        gray: &[*mut GcHeader],
        again: &[*mut GcHeader],
    ) {
        let mut m = self.gc_begin_propagate();
        // SAFETY: the caller's contract; only flag bytes are written, and
        // a black object's children are marked as a traversal marks them
        unsafe {
            // every colour first: a black object's children are then
            // found coloured, all but luna's own
            for &h in black {
                (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | BLACK);
            }
            for &h in gray.iter().chain(again) {
                (*h).flags = (*h).with_slow((*h).flags & !COLOR_BITS);
            }
            for &h in again {
                if self.grayagain.push(h).is_err() {
                    self.gray_overflow = true;
                }
            }
            for &h in black {
                crate::runtime::heap::mark::trace_children(h, &mut m);
            }
            for &h in gray.iter().rev() {
                m.push_gray(h);
            }
        }
        self.stash_marker(m);
    }

    /// Enter the sweep of the list of objects (5.5's `GCSswpallgc`) with
    /// `unswept` (alive or not) still to be swept, in that order, after
    /// every other object, which the sweep has made the current white.
    ///
    /// # Safety
    /// As [`Heap::acct_enter_midcycle`]'s.
    pub(crate) unsafe fn acct_enter_midsweep(&mut self, unswept: &[(*mut GcHeader, bool)]) {
        let tail: std::collections::HashSet<usize> =
            unswept.iter().map(|&(h, _)| h as usize).collect();
        // the marking is over: the white of the objects it found dead is
        // the other one now, as `atomic` leaves it
        self.current_white ^= WHITE_BITS;
        let (live, dead) = (self.current_white, self.current_white ^ WHITE_BITS);
        let mut order = Vec::new();
        // SAFETY: the caller's contract; the links of `all` lead to live
        // objects, and only flag bytes and links are written
        unsafe {
            let mut cur = self.all;
            while !cur.is_null() {
                if !tail.contains(&(cur as usize)) {
                    (*cur).flags = (*cur).with_slow(((*cur).flags & !COLOR_BITS) | live);
                    order.push(cur);
                }
                cur = (*cur).next;
            }
            let swept = order.len();
            for &(h, alive) in unswept {
                let c = if alive { BLACK } else { dead };
                (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | c);
                order.push(h);
            }
            for w in order.windows(2) {
                (*w[0]).next = w[1];
            }
            if let Some(&last) = order.last() {
                (*last).next = ptr::null_mut();
            }
            self.all = order.first().copied().unwrap_or(ptr::null_mut());
            self.sweep_prev = if swept == 0 {
                ptr::null_mut()
            } else {
                order[swept - 1]
            };
        }
        // every object is rooted here, the newest first on the list
        self.safe_point();
        self.phase = GcPhase::Sweep;
    }
}
