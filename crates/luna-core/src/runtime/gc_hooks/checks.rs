//! The hooks of the checks (`gc-verify`, and the slow bits of debug
//! builds): the invariants checked after the atomic step and each sweep,
//! the freed addresses the read probes know.

use crate::runtime::heap::{GcHeader, Heap};
use crate::runtime::{Table, Value};

impl Heap {
    /// The object `h` is freed: gc-verify notes it for the read probes.
    #[inline(always)]
    pub(crate) fn hook_freed(&mut self, h: *mut GcHeader) {
        #[cfg(feature = "gc-verify")]
        self.note_freed(h);
        let _ = h;
    }

    /// The freed table `t` is made again from the pool: no longer freed.
    #[inline(always)]
    pub(crate) fn hook_reborn(&mut self, h: *mut GcHeader) {
        #[cfg(feature = "gc-verify")]
        self.forget_freed(h);
        let _ = h;
    }

    /// gc-verify: `h`, about to be held by `what`, is not one the sweep
    /// under way frees.
    #[inline(always)]
    pub(crate) fn check_not_dying(&self, h: *const GcHeader, what: &str) {
        #[cfg(feature = "gc-verify")]
        self.verify_not_dying(h, what);
        let _ = (h, what);
    }

    /// gc-verify: after the atomic step, the tricolor invariant, black
    /// objects' children, the string count; the slow bits (debug builds
    /// too).
    #[inline(always)]
    pub(crate) fn check_after_atomic(&self, ctx: &'static str) {
        #[cfg(feature = "gc-verify")]
        {
            self.verify_tricolor(ctx);
            self.verify_black_children(ctx);
            self.verify_string_count(ctx);
        }
        self.check_slow_bits(ctx);
    }

    /// gc-verify: nothing reachable was freed (after a sweep, or a step of
    /// one).
    #[inline(always)]
    pub(crate) fn check_no_dangling(&self, ctx: &str) {
        #[cfg(feature = "gc-verify")]
        self.verify_no_dangling(ctx);
        let _ = ctx;
    }

    /// The slow bits agree with the colours (debug builds and gc-verify).
    #[inline(always)]
    pub(crate) fn check_slow_bits(&self, ctx: &str) {
        #[cfg(any(debug_assertions, feature = "gc-verify"))]
        self.verify_slow_bits(ctx);
        let _ = ctx;
    }
}

impl Table {
    /// gc-verify: the key a hash lookup compares, and every node key it
    /// meets, are live.
    #[inline(always)]
    pub(crate) fn hook_find_node(&self, k: Value) {
        #[cfg(feature = "gc-verify")]
        self.verify_find_node_keys(k);
        let _ = k;
    }
}
