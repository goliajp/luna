//! What only the gc-trace build's VM asks of the count.

use super::super::*;

impl Heap {
    /// gc-trace: the count moved by `delta` where the instrumented PUC's
    /// allocation function refused a growth.
    pub(crate) fn adjust_count(&mut self, delta: i64) {
        self.puc.count.bytes = (self.puc.count.bytes as i64 + delta) as usize;
    }
}
