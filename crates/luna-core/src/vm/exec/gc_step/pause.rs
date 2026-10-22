//! The debt and estimate a full collection or a change of mode leaves.

use super::*;

impl Vm {
    /// A full collection ended (`luaC_fullgc`): the cycle at its pause,
    /// the estimate what is left (5.5: what was marked), and the debt
    /// `setpause` sets.
    pub(crate) fn gc_full_pause(&mut self) {
        let d = self.heap.table_dialect;
        let bytes = self.heap.bytes() as i64;
        let p = &mut self.heap.puc.pace;
        p.state = St::Pause;
        p.fin_left = None;
        if d == Dialect::L51 {
            p.dept = 0;
        }
        // 5.2 keeps what the cycle traversed
        if !matches!(d, Dialect::L52 | Dialect::L55) {
            p.estimate = bytes;
        }
        self.gc_full_repause();
    }

    /// The pause a full collection sets (`setpause`, 5.1 `setthreshold`)
    /// from the estimate it left, with the count as it is now: after the
    /// finalizers it called but 5.2's.
    pub(crate) fn gc_full_repause(&mut self) {
        let d = self.heap.table_dialect;
        let bytes = self.heap.bytes();
        let pause = self.gc_params.get(Param::Pause);
        let p = &mut self.heap.puc.pace;
        match d {
            Dialect::L51 => p.set_threshold(threshold_51(p.estimate, pause)),
            Dialect::L55 => {
                let threshold = self.gc_params.apply(Param::Pause, p.estimate);
                p.set_debt(d, bytes, pause_debt_55(bytes, threshold));
            }
            _ => {
                // 5.2's pause is from the count, its estimate what was traversed
                let e = if d == Dialect::L52 {
                    bytes as i64
                } else {
                    p.estimate
                };
                p.set_debt(d, bytes, pause_debt(d, bytes, e, pause));
            }
        }
        self.heap.refresh_gc_limit();
    }

    /// Generational mode entered (`entergen`): the debt before the first
    /// minor collection (`setminordebt`), 5.4 a share of the count, 5.5
    /// of what was marked.
    pub(crate) fn gc_minor_debt(&mut self) {
        let d = self.heap.table_dialect;
        let bytes = self.heap.bytes();
        let debt = match d {
            Dialect::L54 => -((bytes as i64 / 100) * self.gc_params.get(Param::MinorMul)),
            Dialect::L55 => self
                .gc_params
                .apply(Param::MinorMul, self.heap.puc.pace.estimate),
            _ => return,
        };
        self.heap.puc.pace.set_debt(d, bytes, debt);
        if d == Dialect::L55 {
            // `GCmarked` counts the bytes that become old from here
            self.heap.puc.pace.estimate = 0;
        }
        self.heap.refresh_gc_limit();
    }

    /// Back to incremental mode: 5.4 waits for its next cycle (`enterinc`);
    /// 5.5 sweeps first, a step's size away (`minor2inc`).
    pub(crate) fn gc_leave_generational(&mut self) {
        if self.heap.table_dialect != Dialect::L55 || !self.heap.gc_phase_is_pause() {
            return;
        }
        self.heap.puc.pace.count_marked = false;
        self.heap.gc_sweep_all_live();
        self.heap.puc.pace.state = St::SweepAll;
        let bytes = self.heap.bytes();
        let debt = self.gc_params.apply(Param::StepSize, 100);
        self.heap.puc.pace.set_debt(Dialect::L55, bytes, debt);
        self.heap.refresh_gc_limit();
    }
}
