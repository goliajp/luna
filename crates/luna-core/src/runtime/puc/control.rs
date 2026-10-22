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

    /// 5.1's threshold (`GCthreshold`).
    #[inline]
    pub(crate) fn gc_threshold(&self) -> i64 {
        self.puc.pace.threshold()
    }

    #[inline]
    pub(crate) fn set_gc_threshold(&mut self, t: i64) {
        self.puc.pace.set_threshold(t);
    }

    /// Whether this cycle's propagation shrank thread `h` (5.3), or its
    /// sweep kept it (5.2).
    #[inline]
    pub(crate) fn shrunk_this_cycle(&self, h: *mut GcHeader) -> bool {
        self.puc.shrunk.contains(&h)
    }

    /// The threads [`Heap::shrunk_this_cycle`] names, taken (5.2's sweep
    /// shrinks them), to hand back emptied with [`Heap::put_back_shrunk`].
    #[inline]
    pub(crate) fn take_shrunk(&mut self) -> Vec<*mut GcHeader> {
        std::mem::take(&mut self.puc.shrunk)
    }

    #[inline]
    pub(crate) fn put_back_shrunk(&mut self, mut v: Vec<*mut GcHeader>) {
        v.clear();
        self.puc.shrunk = v;
    }

    /// A cycle restarts: no thread shrunk yet.
    #[inline]
    pub(crate) fn forget_shrunk(&mut self) {
        self.puc.shrunk.clear();
    }
}
