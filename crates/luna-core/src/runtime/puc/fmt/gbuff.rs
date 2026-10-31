//! 5.1 and 5.2's buffer for concatenations (`G(L)->buff`).

use super::*;
use crate::runtime::mem::LVec;

/// The buffer: a block of its size, which the allocation function sees
/// grow, shrink and go as PUC's does, and may refuse.
#[derive(Default)]
pub(in crate::runtime) struct Gbuff(pub(in crate::runtime::puc) Option<LVec<u8>>);

impl Gbuff {
    /// `buff->buffsize`
    pub(in crate::runtime::puc) fn size(&self) -> usize {
        self.0.as_ref().map_or(0, LVec::capacity)
    }
}

impl Heap {
    /// The buffer set to `size` bytes by `checkSizes`, smaller (5.1 halves
    /// it, 5.2 frees it).
    pub(in crate::runtime::puc) fn shrink_gbuff(&mut self, size: usize) {
        let old = self.puc.records.gbuff.size();
        self.shrink_gbuff_to(size);
        self.acct_part(old, self.puc.records.gbuff.size());
    }

    /// The buffer's block made `size` bytes, smaller, uncounted.
    pub(in crate::runtime::puc) fn shrink_gbuff_to(&mut self, size: usize) {
        let Some(b) = self.puc.records.gbuff.0.as_mut() else {
            return;
        };
        if size == 0 {
            self.puc.records.gbuff.0 = None;
        } else {
            // the bytes in use decide what `shrink_to_fit` leaves; within
            // the block, nothing is allocated
            b.resize(size, 0).expect("smaller than the block");
            b.shrink_to_fit();
            b.clear();
        }
    }
}
