//! What the VM asks of PUC's collector outside its steps: the pacing
//! parameters `collectgarbage` reads and writes, the threshold a 5.1
//! finalizer runs under, the threads a cycle shrank.

use super::*;

impl Heap {
    /// `collectgarbage`'s pacing parameters.
    #[inline]
    pub(crate) fn gc_params(&self) -> &GcParams {
        &self.puc.params
    }

    #[inline]
    pub(crate) fn gc_params_mut(&mut self) -> &mut GcParams {
        &mut self.puc.params
    }

    /// There may be objects to be finalized (PUC `gcfinnum = 1`).
    #[inline]
    pub(crate) fn finalizers_due(&mut self) {
        self.puc.pace.fin_num = 1;
    }

    /// 5.1's `GCTM` runs a finalizer: under a threshold of twice the
    /// count. The threshold it had, for [`Heap::fin51_threshold_back`].
    #[inline]
    pub(in crate::runtime::puc) fn fin51_threshold(&mut self) -> i64 {
        let saved = self.puc.pace.threshold();
        self.puc.pace.set_threshold(2 * self.puc.count.bytes as i64);
        saved
    }

    /// The finalizer returned: the threshold it ran under is `saved` again.
    #[inline]
    pub(in crate::runtime::puc) fn fin51_threshold_back(&mut self, saved: i64) {
        self.puc.pace.set_threshold(saved);
    }
}
