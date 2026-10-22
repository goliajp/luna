//! The accounting trace's refusals (`gc-trace` with `LUNA_GC_REFUSE`): the
//! instrumented PUC's allocation function refuses a growth, which luna
//! answers where PUC does.

use crate::runtime::Table;
use crate::runtime::heap::Heap;

impl Heap {
    /// gc-trace: the instrumented PUC refuses to allocate a long string of
    /// `len` bytes here.
    #[inline(always)]
    pub(crate) fn refuse_string_now(&mut self, len: usize) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.refuse_string(len);
        #[cfg(not(feature = "gc-trace"))]
        {
            let _ = len;
            false
        }
    }

    /// gc-trace: the instrumented PUC refuses to resize table `t` (one of
    /// PUC's) from `old_a` array slots to `new_a` and `new_h` nodes.
    #[inline(always)]
    pub(crate) fn refuse_table_now(
        &mut self,
        t: &Table,
        old_a: usize,
        new_a: usize,
        new_h: usize,
    ) -> bool {
        #[cfg(feature = "gc-trace")]
        return !self.is_own_table(t) && self.refuse_table(old_a, new_a, new_h);
        #[cfg(not(feature = "gc-trace"))]
        {
            let _ = (t, old_a, new_a, new_h);
            false
        }
    }

    /// gc-trace: a growth the instrumented PUC refused waits already.
    #[inline(always)]
    pub(crate) fn refusal_waits(&self) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.refused();
        #[cfg(not(feature = "gc-trace"))]
        false
    }

    /// gc-trace: the bytes counted since a growth the instrumented PUC
    /// refused, when it refused one since `waited` was read.
    #[inline(always)]
    pub(crate) fn take_new_refusal(&mut self, waited: bool) -> Option<i64> {
        #[cfg(feature = "gc-trace")]
        if !waited && self.hooks.trace.refused() {
            return Some(self.hooks.trace.take_refused().1);
        }
        let _ = waited;
        None
    }

    /// gc-trace: the stack top an emergency collection answering a refusal
    /// takes, where the VM cannot tell it.
    #[inline(always)]
    pub(crate) fn trace_alloc_top(&self) -> Option<u32> {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.alloc_top;
        #[cfg(not(feature = "gc-trace"))]
        None
    }

    /// gc-trace: the instrumented PUC refuses a box's block growing from
    /// `old` to `new` bytes.
    #[inline(always)]
    pub(crate) fn refuse_box_now(&mut self, old: usize, new: usize) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.hooks.trace.refuse_now(old, new);
        #[cfg(not(feature = "gc-trace"))]
        {
            let _ = (old, new);
            false
        }
    }
}
