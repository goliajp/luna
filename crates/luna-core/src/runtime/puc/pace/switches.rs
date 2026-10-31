//! The collector's pacing and its stop switches, as PUC keeps them: the
//! count against the next step, `collectgarbage("stop")` and 5.4's and
//! 5.5's stop while a finalizer runs. The VM and the libraries reach them
//! through the model's facade (`api`).

use crate::runtime::puc::*;

/// The collector's run at some point (see [`Heap::gc_run_mark`]).
#[must_use]
pub(in crate::runtime::puc) struct RunMark {
    stopped: bool,
    fin_stop: bool,
}

impl Heap {
    /// 5.1: an explicit step sets the threshold that `stop` had put out of
    /// reach, so the collector runs on its own again after it.
    pub(in crate::runtime::puc) fn gc_unstop_51(&mut self) {
        self.puc.stopped = false;
    }

    /// The stop flag put back as it was, with no debt set (PUC `GCTM`
    /// restoring `gcrunning`).
    pub(in crate::runtime::puc) fn gc_restore_stopped(&mut self, stopped: bool) {
        self.puc.stopped = stopped;
        self.refresh_gc_limit();
    }

    /// Whether PUC's `luaC_step` runs a step (`gcrunning`): not while the
    /// collector is stopped, nor while a 5.4 or 5.5 finalizer runs.
    pub(in crate::runtime::puc) fn gc_steps_run(&self) -> bool {
        !self.puc.stopped && !self.puc.fin_stop
    }

    /// Whether the collector runs as it does now (stopped, and stopped for
    /// a 5.4 or 5.5 finalizer), to return to with [`Heap::gc_restore_run`].
    #[inline]
    pub(in crate::runtime::puc) fn gc_run_mark(&self) -> RunMark {
        RunMark {
            stopped: self.puc.stopped,
            fin_stop: self.puc.fin_stop,
        }
    }

    /// Back to the collector's run at `m`.
    #[inline]
    pub(in crate::runtime::puc) fn gc_restore_run(&mut self, m: RunMark) {
        self.puc.fin_stop = m.fin_stop;
        self.gc_restore_stopped(m.stopped);
    }

    /// A 5.4 or 5.5 finalizer runs (`on`), and no step may run, or none
    /// does any more.
    #[inline]
    pub(in crate::runtime::puc) fn gc_set_fin_stop(&mut self, on: bool) {
        self.puc.fin_stop = on;
    }
}
