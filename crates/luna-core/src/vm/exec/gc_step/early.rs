//! The steps of 5.1 and 5.3.

use super::*;

impl Vm {
    pub(super) fn step_53(&mut self) -> Result<(), LuaError> {
        let d = Dialect::L53;
        let s = self.heap.sizes;
        let stepmul = self.gc_params.get(Param::StepMul);
        let mut debt = work_52(d, self.heap.pace.debt(d, self.heap.bytes()), stepmul);
        let floor = -stepsize_52(&s);
        let run = self.gc_running();
        loop {
            if self.heap.pace.state == St::Propagate {
                self.gc_propagate(&mut debt, floor, run);
                if self.heap.gray_is_empty() {
                    self.heap.pace.state = St::Atomic;
                }
            } else {
                debt -= self.singlestep_53()?;
            }
            if !(debt > floor && self.heap.pace.state != St::Pause) {
                break;
            }
        }
        let bytes = self.heap.bytes();
        if self.heap.pace.state == St::Pause {
            let pause = self.gc_params.get(Param::Pause);
            let debt = pause_debt(d, bytes, self.heap.pace.estimate, pause);
            self.heap.pace.set_debt(d, bytes, debt);
        } else {
            self.heap
                .pace
                .set_debt(d, bytes, debt_after_52(debt, stepmul));
            self.run_a_few_finalizers()?;
        }
        Ok(())
    }

    /// 5.3 `runafewfinalizers`: `gcfinnum` of them, twice as many the
    /// next time while some are left.
    fn run_a_few_finalizers(&mut self) -> Result<i64, LuaError> {
        let mut i = 0;
        while self.heap.has_tobefnz() && i < self.heap.pace.fin_num {
            self.gc_tm(true)?;
            i += 1;
        }
        self.heap.pace.fin_num = if self.heap.has_tobefnz() {
            self.heap.pace.fin_num * 2
        } else {
            0
        };
        Ok(i64::from(i))
    }

    pub(super) fn singlestep_53(&mut self) -> Result<i64, LuaError> {
        let s = self.heap.sizes;
        let cost = sweepmax_52(&s) * sweepcost_52(&s);
        Ok(match self.heap.pace.state {
            St::Pause => {
                self.gc_restart();
                (self.heap.puc.strt_size * s.ptr) as i64
            }
            St::Propagate => {
                self.heap.pace.state = St::Atomic;
                0
            }
            St::Atomic => {
                let (work, _) = self.gc_atomic();
                self.heap.pace.state = St::SweepAll;
                self.heap.pace.estimate = self.heap.bytes() as i64;
                work
            }
            St::SweepAll => match self.sweep_quota(sweepmax_52(&s)) {
                Some((_, freed)) if self.heap.gc_phase_is_sweep() => {
                    self.heap.pace.estimate -= freed;
                    cost
                }
                r => {
                    if let Some((_, freed)) = r {
                        self.heap.pace.estimate -= freed;
                    }
                    self.heap.pace.state = St::SweepFinobj;
                    self.heap.pace.fin_left = Some(self.heap.finobj_count());
                    0
                }
            },
            St::SweepFinobj | St::SweepTobefnz => {
                let finobj = self.heap.pace.state == St::SweepFinobj;
                if sweep_other(&mut self.heap.pace.fin_left, sweepmax_52(&s))
                    && self.heap.pace.fin_left.is_some()
                {
                    return Ok(cost);
                }
                self.heap.pace.state = if finobj {
                    self.heap.pace.fin_left = Some(self.heap.tobefnz_count());
                    St::SweepTobefnz
                } else {
                    St::SweepEnd
                };
                0
            }
            St::SweepEnd => {
                self.heap.acct_check_sizes();
                self.heap.pace.state = St::CallFin;
                0
            }
            _ => {
                // emergency collections run no finalizer (they are full
                // ones, which never step here)
                if self.heap.has_tobefnz() {
                    let n = self.run_a_few_finalizers()?;
                    return Ok(n * sweepcost_52(&s));
                }
                self.heap.pace.state = St::Pause;
                0
            }
        })
    }

    pub(super) fn step_51(&mut self) -> Result<(), LuaError> {
        let mut lim = step_limit_51(self.gc_params.get(Param::StepMul));
        let bytes = self.heap.bytes() as i64;
        let p = &mut self.heap.pace;
        p.dept += bytes - p.threshold();
        let run = self.gc_running();
        loop {
            if self.heap.pace.state == St::Propagate {
                if self.gc_propagate(&mut lim, 0, run) {
                    self.atomic_51();
                }
            } else {
                lim -= self.singlestep_51()?;
            }
            if self.heap.pace.state == St::Pause || lim <= 0 {
                break;
            }
        }
        let bytes = self.heap.bytes() as i64;
        let p = &mut self.heap.pace;
        if p.state != St::Pause {
            if p.dept < STEPSIZE_51 {
                p.set_threshold(bytes + STEPSIZE_51);
            } else {
                p.dept -= STEPSIZE_51;
                p.set_threshold(bytes);
            }
        } else {
            let pause = self.gc_params.get(Param::Pause);
            p.set_threshold(threshold_51(p.estimate, pause));
        }
        Ok(())
    }

    /// 5.1's `atomic`, from the step that finds no gray object.
    fn atomic_51(&mut self) {
        self.gc_atomic();
        self.heap.pace.state = St::SweepString;
        // `totalbytes - udsize`: what is to be finalized is not counted
        self.heap.pace.estimate = self.heap.bytes() as i64 - self.heap.puc.udsize;
        self.heap.pace.strt_swept = 0;
    }

    pub(super) fn singlestep_51(&mut self) -> Result<i64, LuaError> {
        Ok(match self.heap.pace.state {
            St::Pause => {
                self.gc_restart();
                0
            }
            St::Propagate => {
                self.atomic_51();
                0
            }
            St::SweepString => {
                self.heap.pace.strt_swept += 1;
                let freed = self.heap.sweep_strings_upto(self.heap.pace.strt_swept);
                self.heap.pace.estimate -= freed;
                if self.heap.pace.strt_swept >= self.heap.puc.strt_size as i64 {
                    self.heap.end_string_sweep();
                    self.heap.pace.state = St::SweepAll;
                }
                SWEEPCOST_51
            }
            St::SweepAll => {
                // what `checkSizes` frees at the end is taken off too
                let old = self.heap.bytes() as i64;
                self.sweep_quota(SWEEPMAX_51);
                if !self.heap.gc_phase_is_sweep() {
                    self.heap.acct_check_sizes();
                    self.heap.pace.state = St::Finalize;
                }
                self.heap.pace.estimate -= old - self.heap.bytes() as i64;
                SWEEPMAX_51 * SWEEPCOST_51
            }
            _ => {
                if self.heap.has_tobefnz() {
                    self.gc_tm(true)?;
                    let p = &mut self.heap.pace;
                    if p.estimate > FINALIZECOST_51 {
                        p.estimate -= FINALIZECOST_51;
                    }
                    return Ok(FINALIZECOST_51);
                }
                self.heap.pace.state = St::Pause;
                self.heap.pace.dept = 0;
                0
            }
        })
    }
}
