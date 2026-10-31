//! Driving PUC's collector: its parameters and pace, its steps, and the
//! records its marking and sweep keep (5.1's environments, open upvalues).

use super::super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::step::CiCount;

/// How the collector ran before a finalizer (see
/// [`Heap::finalizer_runs`]).
#[must_use = "the collector stays stopped until the token goes back to `Heap::finalizer_returned`"]
pub(crate) struct FinalizerStop {
    stopped: bool,
    fin_stop: bool,
    threshold: i64,
}

impl Heap {
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

impl Heap {
    /// What a memory cap is held against: the bytes the allocation context
    /// has handed out, when it counts them (a host function or a policy),
    /// else PUC's count ([`Heap::bytes`]).
    pub(crate) fn cap_bytes(&self) -> usize {
        self.mem_ctx().in_use().unwrap_or(self.puc.count.bytes)
    }

    /// [`Heap::gc_due`] once `more` bytes more are counted.
    #[inline]
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

    /// The count at which the next step falls due, from the pace. 5.1
    /// stops the collector with an unreachable threshold; the later
    /// dialects still step then, to put the debt back.
    pub(crate) fn refresh_gc_limit(&mut self) {
        self.puc_set_limit(self.puc.stopped && self.puc_rules().stop_unreachable);
    }

    /// A finalizer is about to run (`GCTM`): the collector stops as PUC's
    /// does, 5.1 under a threshold twice the count, 5.2 and 5.3 not
    /// running, 5.4 and 5.5 stopped for the finalizer (`GCSTPGC`). What
    /// [`Heap::finalizer_returned`] puts back.
    pub(crate) fn finalizer_runs(&mut self) -> FinalizerStop {
        let stop = FinalizerStop {
            stopped: self.puc.stopped,
            fin_stop: self.puc.fin_stop,
            threshold: match self.puc.dialect {
                Dialect::L51 => self.fin51_threshold(),
                _ => 0,
            },
        };
        match self.puc.dialect {
            Dialect::L51 => {}
            Dialect::L52 | Dialect::L53 => self.puc.stopped = true,
            _ => self.puc.fin_stop = true,
        }
        self.refresh_gc_limit();
        stop
    }

    /// The finalizer returned, `ok` or raising: the collector runs as
    /// before it; a 5.1 error leaves the threshold it ran under.
    pub(crate) fn finalizer_returned(&mut self, stop: FinalizerStop, ok: bool) {
        match self.puc.dialect {
            Dialect::L51 => {
                if ok {
                    self.fin51_threshold_back(stop.threshold);
                    self.puc.stopped = stop.stopped;
                }
            }
            Dialect::L52 | Dialect::L53 => self.puc.stopped = stop.stopped,
            _ => self.puc.fin_stop = stop.fin_stop,
        }
        self.refresh_gc_limit();
    }

    /// Whether an allocation that failed may run an emergency collection:
    /// never in 5.1; in 5.2 only while the collector runs (`gcrunning`);
    /// from 5.3 on, always.
    pub(crate) fn emergency_allowed(&self) -> bool {
        match self.puc.dialect {
            Dialect::L51 => false,
            Dialect::L52 => !self.puc.stopped,
            _ => true,
        }
    }

    /// 5.2's full collection calls the pending finalizers first; the
    /// others clear the running stack's dead slots.
    pub(crate) fn full_calls_pending_first(&self) -> bool {
        self.puc.dialect == Dialect::L52
    }

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

    /// Whether the standalone interpreter restarts the collector in
    /// generational mode before it runs anything (5.4 on `lua.c`).
    pub(crate) fn lua_c_starts_generational(&self) -> bool {
        self.puc.dialect >= Dialect::L54
    }

    /// Which `CallInfo`s PUC's traversal of a thread counts.
    pub(crate) fn puc_ci_count(&self) -> CiCount {
        match self.puc.dialect {
            Dialect::L51 => CiCount::Array51,
            Dialect::L52 => CiCount::InUse,
            _ => CiCount::Allocated,
        }
    }

    /// The light function the opening made as function `id`.
    pub(crate) fn light(&self, id: usize) -> Option<Gc<crate::runtime::NativeClosure>> {
        self.puc.opening.lights.get(id).copied().flatten()
    }

    /// Function `id` of the opening's `count` is the light function `g`:
    /// one value however often pushed.
    pub(crate) fn set_light(
        &mut self,
        id: usize,
        count: usize,
        g: Gc<crate::runtime::NativeClosure>,
    ) {
        let l = &mut self.puc.opening.lights;
        if l.len() <= id {
            l.resize(count.max(id + 1), None);
        }
        l[id] = Some(g);
    }

    /// Whether the count has reached the point a step falls due at: the
    /// comparison compiled code makes with the words of [`Heap::due_words`].
    #[inline]
    pub(crate) fn due_reached(&self) -> bool {
        self.puc_due_after(0)
    }

    /// The two words a safe point compares, `bytes` and `gc_limit`
    /// [`GC_LIMIT_OFFSET`] bytes after it, for compiled code to compare
    /// itself: no step is due while `bytes < gc_limit`.
    ///
    /// [`GC_LIMIT_OFFSET`]: crate::runtime::puc::GC_LIMIT_OFFSET
    pub(crate) fn due_words(&self) -> *const usize {
        std::ptr::from_ref(&self.puc.count).cast()
    }
}
