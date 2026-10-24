//! The collector's pacing and its stop switches, as PUC keeps them: the
//! count against the next step, `collectgarbage("stop")` and 5.4's and
//! 5.5's stop while a finalizer runs. The VM and the libraries read and
//! write them only through these methods.

use super::*;

/// The collector's run at some point (see [`Heap::gc_run_mark`]).
#[must_use]
pub(crate) struct RunMark {
    stopped: bool,
    fin_stop: bool,
}

impl Heap {
    /// Approximate heap size in bytes.
    pub fn bytes(&self) -> usize {
        self.puc.bytes
    }

    /// What a memory cap is held against: the bytes the allocation context
    /// has handed out, when it counts them (a host function or a policy),
    /// else PUC's count ([`Heap::bytes`]).
    pub fn cap_bytes(&self) -> usize {
        self.mem_ctx().in_use().unwrap_or(self.puc.bytes)
    }

    /// Whether allocation has crossed the auto-GC threshold (cheap safe-point
    /// check for the interpreter loop).
    #[inline(always)]
    pub fn gc_due(&self) -> bool {
        self.gc_due_after(0)
    }

    /// [`Heap::gc_due`] once `more` bytes more are counted.
    pub(crate) fn gc_due_after(&self, more: usize) -> bool {
        self.puc_due_after(more) && !self.no_gc() && !self.no_auto()
    }

    /// `collectgarbage("stop"/"restart")`: suspend or resume auto-GC.
    pub(crate) fn gc_is_stopped(&self) -> bool {
        self.puc.stopped
    }

    /// `collectgarbage("stop")` / `"restart"`. A restart makes a step due
    /// at once (5.1 `GCthreshold = totalbytes`, later `luaE_setdebt(g, 0)`).
    pub(crate) fn gc_set_stopped(&mut self, stopped: bool) {
        self.puc.stopped = stopped;
        if !stopped {
            self.puc_restart_due();
        }
        self.refresh_gc_limit();
    }

    /// 5.1: an explicit step sets the threshold that `stop` had put out of
    /// reach, so the collector runs on its own again after it.
    pub(in crate::runtime::puc) fn gc_unstop_51(&mut self) {
        self.puc.stopped = false;
    }

    /// The stop flag put back as it was, with no debt set (PUC `GCTM`
    /// restoring `gcrunning`).
    pub(crate) fn gc_restore_stopped(&mut self, stopped: bool) {
        self.puc.stopped = stopped;
        self.refresh_gc_limit();
    }

    /// Whether PUC's `luaC_step` runs a step (`gcrunning`): not while the
    /// collector is stopped, nor while a 5.4 or 5.5 finalizer runs.
    pub(in crate::runtime::puc) fn gc_steps_run(&self) -> bool {
        !self.puc.stopped && !self.puc.fin_stop
    }

    /// The count at which the next step falls due, from the pace. 5.1
    /// stops the collector with an unreachable threshold; the later
    /// dialects still step then, to put the debt back.
    pub(crate) fn refresh_gc_limit(&mut self) {
        self.puc_set_limit(self.puc.stopped && self.puc_rules().stop_unreachable);
    }

    /// A cycle ended with the count at what it is now: the pause before
    /// the next one, `pause` in percent of what survived (PUC `setpause`;
    /// 5.1 `setthreshold`).
    pub fn rearm_gc_pause(&mut self, pause: i64) {
        self.puc_pause(pause);
        self.refresh_gc_limit();
    }

    /// [`Heap::rearm_gc_pause`] with PUC's usual pause (200%).
    pub fn rearm_gc(&mut self) {
        self.rearm_gc_pause(200);
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

    /// Whether a 5.4 or 5.5 finalizer runs, which no step may interrupt.
    #[inline]
    pub(crate) fn gc_fin_stop(&self) -> bool {
        self.puc.fin_stop
    }

    /// A 5.4 or 5.5 finalizer runs (`on`), and no step may run, or none
    /// does any more.
    #[inline]
    pub(crate) fn gc_set_fin_stop(&mut self, on: bool) {
        self.puc.fin_stop = on;
    }
}
