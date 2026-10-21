//! `gc-stress`: a full collection at every checkpoint, or, for a program
//! too slow for that, at every `n`th one.

use super::*;

impl Vm {
    /// From now on collect at every `every`th checkpoint only, the first
    /// after `offset + 1` more (`offset < every`), instead of at each one.
    /// Different offsets over runs cover different checkpoints.
    pub fn gc_stress_interval(&mut self, every: u64, offset: u64) {
        assert!(offset < every, "an offset within the interval");
        self.stress_every = (every, offset + 1);
    }

    /// The checkpoint at `live_top` with the collector running.
    pub(super) fn stress_checkpoint(&mut self, live_top: u32) {
        self.heap.safe_point();
        self.stress_every.1 -= 1;
        if self.stress_every.1 == 0 {
            self.stress_every.1 = self.stress_every.0;
            self.gc_top = live_top;
            self.collect_garbage();
        }
    }
}
