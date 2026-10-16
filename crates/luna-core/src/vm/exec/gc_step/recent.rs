//! The steps of 5.4 and 5.5.

use super::*;

impl Vm {
    pub(super) fn step_55(&mut self) {
        let d = Dialect::L55;
        let stepsize = self.gc_params.apply(Param::StepSize, 100);
        let mut work = self
            .gc_params
            .apply(Param::StepMul, stepsize / self.heap.sizes.ptr as i64);
        let fast = work == 0;
        let run = self.gc_running();
        loop {
            if !fast && self.heap.pace.state == St::Propagate {
                if self.heap.propagate_work(&mut work, 0, run) {
                    // the step that finds no gray object
                    self.heap.pace.state = St::EnterAtomic;
                    work -= 1;
                }
            } else {
                let r = self.singlestep_55(fast);
                if r == STEP2PAUSE || (r == ATOMICSTEP && !fast) {
                    break;
                }
                work -= r;
            }
            if !(fast || work > 0) {
                break;
            }
        }
        let bytes = self.heap.bytes();
        let debt = if self.heap.pace.state == St::Pause {
            let threshold = self.gc_params.apply(Param::Pause, self.heap.pace.estimate);
            pause_debt_55(bytes, threshold)
        } else {
            stepsize
        };
        self.heap.pace.set_debt(d, bytes, debt);
    }

    fn singlestep_55(&mut self, fast: bool) -> i64 {
        match self.heap.pace.state {
            St::Pause => {
                self.gc_restart();
                1
            }
            St::Propagate => {
                if fast {
                    self.heap.pace.state = St::EnterAtomic;
                    return 1;
                }
                let run = self.gc_running();
                let mut w = 1;
                if self.heap.propagate_work(&mut w, 0, run) {
                    self.heap.pace.state = St::EnterAtomic;
                    return 1;
                }
                1 - w
            }
            St::EnterAtomic => {
                self.gc_atomic();
                self.heap.pace.state = St::SweepAll;
                ATOMICSTEP
            }
            _ => self.sweep_step_55(fast),
        }
    }

    fn sweep_step_55(&mut self, fast: bool) -> i64 {
        let quota = if fast { i64::MAX } else { SWEEPMAX_55 };
        match self.heap.pace.state {
            St::SweepAll => {
                if self.sweep_quota(quota).is_none() {
                    self.heap.pace.state = St::SweepFinobj;
                    self.heap.pace.fin_left = Some(self.heap.finobj_count());
                }
                SWEEPMAX_55
            }
            St::SweepFinobj => {
                if !sweep_other(&mut self.heap.pace.fin_left, quota) {
                    self.heap.pace.state = St::SweepTobefnz;
                    self.heap.pace.fin_left = Some(self.heap.tobefnz_count());
                }
                SWEEPMAX_55
            }
            St::SweepTobefnz => {
                if !sweep_other(&mut self.heap.pace.fin_left, quota) {
                    self.heap.pace.state = St::SweepEnd;
                }
                SWEEPMAX_55
            }
            St::SweepEnd => {
                self.heap.acct_check_sizes();
                self.heap.pace.state = St::CallFin;
                SWEEPMAX_55
            }
            _ => {
                // the finalizers ran when the cycle found them
                self.heap.pace.state = St::Pause;
                STEP2PAUSE
            }
        }
    }

    pub(super) fn step_54(&mut self) {
        let d = Dialect::L54;
        let s = self.heap.sizes;
        let stepmul = (self.gc_params.get(Param::StepMul)) | 1;
        let debt0 = self.heap.pace.debt(d, self.heap.bytes());
        let (mut debt, stepsize) = work_54(&s, debt0, stepmul, self.gc_params.get(Param::StepSize));
        let run = self.gc_running();
        loop {
            if self.heap.pace.state == St::Propagate {
                if self.heap.propagate_work(&mut debt, -stepsize, run) {
                    self.heap.pace.state = St::EnterAtomic;
                }
            } else {
                debt -= self.singlestep_54();
            }
            if !(debt > -stepsize && self.heap.pace.state != St::Pause) {
                break;
            }
        }
        let bytes = self.heap.bytes();
        if self.heap.pace.state == St::Pause {
            let pause = self.gc_params.get(Param::Pause);
            let debt = pause_debt(d, bytes, self.heap.pace.estimate, pause);
            self.heap.pace.set_debt(d, bytes, debt);
        } else {
            let debt = debt_after_54(&s, debt, stepmul);
            self.heap.pace.set_debt(d, bytes, debt);
        }
    }

    fn singlestep_54(&mut self) -> i64 {
        match self.heap.pace.state {
            St::Pause => {
                self.gc_restart();
                1
            }
            St::Propagate => {
                self.heap.pace.state = St::EnterAtomic;
                0
            }
            St::EnterAtomic => {
                let (work, _) = self.gc_atomic();
                self.heap.pace.state = St::SweepAll;
                self.heap.pace.estimate = self.heap.bytes() as i64;
                work
            }
            St::SweepAll => match self.sweep_quota(SWEEPMAX_54) {
                Some((n, freed)) => {
                    self.heap.pace.estimate -= freed;
                    n
                }
                None => {
                    self.heap.pace.state = St::SweepFinobj;
                    self.heap.pace.fin_left = Some(self.heap.finobj_count());
                    0
                }
            },
            St::SweepFinobj | St::SweepTobefnz => {
                let finobj = self.heap.pace.state == St::SweepFinobj;
                match self.heap.pace.fin_left {
                    Some(n) => {
                        sweep_other(&mut self.heap.pace.fin_left, SWEEPMAX_54);
                        n.min(SWEEPMAX_54)
                    }
                    None => {
                        self.heap.pace.state = if finobj {
                            self.heap.pace.fin_left = Some(self.heap.tobefnz_count());
                            St::SweepTobefnz
                        } else {
                            St::SweepEnd
                        };
                        0
                    }
                }
            }
            St::SweepEnd => {
                self.heap.acct_check_sizes();
                self.heap.pace.state = St::CallFin;
                0
            }
            _ => {
                self.heap.pace.state = St::Pause;
                0
            }
        }
    }
}
