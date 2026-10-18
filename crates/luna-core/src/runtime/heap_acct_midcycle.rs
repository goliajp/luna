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
}
