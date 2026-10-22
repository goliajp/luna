//! The atomic step of an incremental cycle and the work PUC's `atomic`
//! counts for what it traverses.

use super::*;

impl Heap {
    /// Conclude a Propagate cycle: drain any residual gray, run the atomic
    /// tail (weak / ephemeron / finalizer / proto-cache / flip), detach `all`
    /// and enter `GcPhase::Sweep`. Releases `propagate`.
    /// PUC `atomic` + `entersweep` transition.
    pub(crate) fn gc_finish_atomic(&mut self) {
        self.gc_finish_atomic_work(None);
    }

    /// [`Heap::gc_finish_atomic`], counting the work PUC's `atomic` counts
    /// for the objects it traverses (with `run` the running thread), and
    /// entering the sweep as the dialect does: 5.1 sweeps nothing yet, 5.3
    /// one object, the others to the first object kept. Returns the work
    /// and how many objects that sweep went over.
    pub(crate) fn gc_finish_atomic_work(&mut self, run: Option<Running>) -> (i64, i64) {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        // any residual gray (caller may not have drained to empty), then
        // what the back barriers sent to gray again (PUC `grayagain`)
        // 5.2 counts none of it: what the roots marked again reach is
        // marked black already or found where it traverses the threads
        // and the barriers' tables again (`retraversegrays`), uncounted;
        // 5.3 stops counting before `grayagain`, the threads among it (the
        // caller counts what the other roots reach)
        let d = self.table_dialect;
        let l52 = d == crate::runtime::table::Dialect::L52;
        let again_free = l52 || d == crate::runtime::table::Dialect::L53;
        let first = self.drain_work(&mut m, run);
        self.regray_again(&mut m);
        let again = self.drain_work(&mut m, run);
        let mut work = if again_free { 0 } else { first + again };
        m.drain_quiet();
        // a thread stays gray while marking (`grayagain`): `atomic`
        // traverses each one marked again
        if let Some(run) = run {
            let mut seen_run = false;
            for i in 0..self.threads.len() {
                let co = self.threads[i];
                let h = co.as_ptr() as *mut GcHeader;
                seen_run |= h == run.h;
                // SAFETY: a listed thread is allocated until the sweep
                unsafe {
                    if !is_white((*h).flags) {
                        let w = self.object_work(h, run);
                        #[cfg(feature = "gc-trace")]
                        self.trace_prop(h, w, run);
                        work += if again_free { 0 } else { w };
                    }
                }
            }
            // the main thread, luna's own object, is on no list
            if !seen_run && !run.h.is_null() {
                // SAFETY: the running thread is allocated
                // SAFETY: as above
                let w = unsafe { self.object_work(run.h, run) };
                #[cfg(feature = "gc-trace")]
                // SAFETY: as above
                unsafe {
                    self.trace_prop(run.h, w, run)
                };
                work += if again_free { 0 } else { w };
            }
        }
        // pre-atomic ephemeron convergence
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: as in `mark_all`: `t` is a marked table from `m.ephemeron`, nothing is freed before the sweep, and the marker holds no other reference to it
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, &mut m) };
                }
                drain_marker(&mut m);
                if !changed {
                    break;
                }
            }
        }
        self.puc.udsize = self.atomic_tail(&mut m, run);
        if self.table_dialect == crate::runtime::table::Dialect::L52 {
            self.color_fixed_strings(0);
        }
        self.settle_sizer(&mut m);
        #[cfg(feature = "gc-trace")]
        self.trace_listed();
        if run.is_some() {
            self.collect_dead_strings();
        } else {
            self.end_string_sweep();
        }
        // PropagateState consumed; the sweep goes over `all` in place
        // (PUC `entersweep`). Anything allocated past this point goes in
        // front of where it is and survives.
        self.propagate = None;
        debug_assert!(
            self.gray.is_empty() && !self.gray_overflow,
            "gray queue not drained at atomic"
        );
        // the newest object at the last safe point may be freed by the
        // sweep: until the next safe point every object counts as young
        self.young = ptr::null_mut();
        self.phase = GcPhase::Sweep;
        self.sweep_prev = ptr::null_mut();
        let swept = match self.table_dialect {
            crate::runtime::table::Dialect::L51 => 0,
            crate::runtime::table::Dialect::L53 => self.sweep_objects(1, false) as i64,
            _ => self.sweep_objects(usize::MAX, true) as i64,
        };
        (work, swept)
    }

    /// Drain `m`, with the work PUC counts for each object when `run`
    /// names the running thread.
    pub(crate) fn drain_work(&mut self, m: &mut Marker, run: Option<Running>) -> i64 {
        let Some(run) = run else {
            drain_marker(m);
            return 0;
        };
        let mut work = 0;
        while let Some(h) = m.pop() {
            m.leaf_bytes = 0;
            // SAFETY: as in `propagate_work`
            unsafe {
                blacken(h, m);
                let w = self.object_work(h, run) + m.leaf_bytes;
                #[cfg(feature = "gc-trace")]
                self.trace_prop(h, w, run);
                work += w;
            }
        }
        work
    }

    /// The objects a back barrier sent to gray, on `m`'s stack to be
    /// traversed again.
    pub(super) fn regray_again(&mut self, m: &mut Marker) {
        while let Some(h) = self.grayagain.pop() {
            m.push_gray(h);
        }
    }
}
