//! The steps of 5.1, 5.2 and 5.3.

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
                self.heap.propagate_work(&mut debt, floor, run);
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

    pub(super) fn step_52(&mut self) -> Result<(), LuaError> {
        let d = Dialect::L52;
        let s = self.heap.sizes;
        let stepmul = self.gc_params.get(Param::StepMul).max(40);
        let mut debt = work_52(d, self.heap.pace.debt(d, self.heap.bytes()), stepmul);
        let floor = -stepsize_52(&s);
        let run = self.gc_running();
        loop {
            if self.heap.pace.state == St::Propagate {
                let before = debt;
                let empty = self.heap.propagate_work(&mut debt, floor, run);
                // what was traversed, for the estimate
                self.heap.pace.estimate += before - debt;
                if empty {
                    debt -= self.atomic_52();
                }
            } else {
                debt -= self.singlestep_52();
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
        }
        // `luaC_forcestep`: a few finalizers, all of them at a pause
        let mut i = 0;
        while self.heap.has_tobefnz() && (i < 4 || self.heap.pace.state == St::Pause) {
            self.gc_tm(true)?;
            i += 1;
        }
        Ok(())
    }

    /// 5.2's step that finds no gray object: the atomic step, the
    /// estimate the memory traversed, and the sweep entered.
    fn atomic_52(&mut self) -> i64 {
        let s = self.heap.sizes;
        let (work, swept) = self.gc_atomic();
        self.heap.pace.estimate += work;
        self.heap.pace.state = St::SweepString;
        self.heap.pace.strt_swept = 0;
        work + swept * sweepcost_52(&s)
    }

    fn singlestep_52(&mut self) -> i64 {
        let s = self.heap.sizes;
        let (max, cost) = (sweepmax_52(&s), sweepcost_52(&s));
        match self.heap.pace.state {
            St::Pause => {
                self.gc_restart();
                let w = (self.heap.puc.strt_size * s.ptr) as i64;
                self.heap.pace.estimate = w;
                w
            }
            St::Propagate => self.atomic_52(),
            St::SweepString => {
                let size = self.heap.puc.strt_size as i64;
                let i = max.min(size - self.heap.pace.strt_swept);
                self.heap.pace.strt_swept += i;
                self.heap.sweep_strings_upto(self.heap.pace.strt_swept);
                if self.heap.pace.strt_swept >= size {
                    self.heap.end_string_sweep();
                    self.heap.pace.state = St::SweepUdata;
                    self.heap.pace.fin_left = Some(self.heap.finobj_count());
                }
                i * cost
            }
            St::SweepUdata => {
                if sweep_other(&mut self.heap.pace.fin_left, max) {
                    return max * cost;
                }
                self.heap.pace.state = St::SweepAll;
                0
            }
            _ => match self.sweep_quota(max) {
                Some(_) => max * cost,
                None => {
                    self.heap.acct_check_sizes();
                    self.heap.pace.state = St::Pause;
                    cost
                }
            },
        }
    }

    pub(super) fn step_51(&mut self) -> Result<(), LuaError> {
        let mut lim = step_limit_51(self.gc_params.get(Param::StepMul));
        let bytes = self.heap.bytes() as i64;
        let p = &mut self.heap.pace;
        p.dept += bytes - p.threshold();
        let run = self.gc_running();
        loop {
            if self.heap.pace.state == St::Propagate {
                if self.heap.propagate_work(&mut lim, 0, run) {
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
        self.heap.pace.estimate = self.heap.bytes() as i64;
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
                if let Some((_, freed)) = self.sweep_quota(SWEEPMAX_51) {
                    self.heap.pace.estimate -= freed;
                }
                if !self.heap.gc_phase_is_sweep() {
                    self.heap.acct_check_sizes();
                    self.heap.pace.state = St::Finalize;
                }
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
