//! What only the gc-trace build's VM asks of the count.

use crate::runtime::heap::Heap;

/// The count as the gc-trace build's refusals move it.
pub(crate) trait PucCountTrace {
    /// The count moved by `delta` where the instrumented PUC's allocation
    /// function refused a growth.
    fn adjust_count(&mut self, delta: i64);
}

impl PucCountTrace for Heap {
    #[inline(always)]
    fn adjust_count(&mut self, delta: i64) {
        Heap::adjust_count(self, delta)
    }
}
