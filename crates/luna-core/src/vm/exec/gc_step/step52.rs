//! The steps of 5.2.

use super::*;

impl Vm {
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
                let empty = self.gc_propagate(&mut debt, floor, run);
                // what was traversed, for the estimate
                self.heap.pace.trav += before - debt;
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

    /// 5.2's `sweepthread` of each coroutine the sweep just kept (the main
    /// thread's comes at the end of the sweep).
    pub(crate) fn shrink_swept_52(&mut self) {
        let mut swept = std::mem::take(&mut self.heap.puc.shrunk);
        for &h in &swept {
            if !self.main_coro.is_some_and(|m| m.as_ptr() as *mut _ == h) {
                self.collector_shrinks_thread(h);
            }
        }
        swept.clear();
        self.heap.puc.shrunk = swept;
    }

    /// The end of 5.2's sweep: the main thread, swept last, is shrunk
    /// (`sweepthread`).
    pub(crate) fn sweep_main_52(&mut self) {
        if self.current.is_none() {
            let inuse = self.stack_in_use();
            self.shrink_running(crate::vm::exec::stack_inuse::Shrink::Collector, inuse);
        } else {
            self.collector_shrinks_main();
        }
    }

    /// 5.2's `luaC_fullgc` from a pause: the whole cycle as single steps,
    /// as PUC runs it, the estimate what it traverses. The objects freed.
    /// `swept`: a cycle was under way, whose sweep has ended.
    pub(crate) fn full_cycle_52(&mut self, swept: bool) -> usize {
        let before = self.heap.live_objects();
        if swept {
            self.shrink_swept_52();
            self.sweep_main_52();
        }
        self.heap.pace.state = St::Pause;
        self.singlestep_52();
        let run = self.gc_running();
        let mut work = i64::MAX;
        while !self.gc_propagate(&mut work, i64::MIN, run) {}
        self.heap.pace.trav += i64::MAX - work;
        self.atomic_52();
        while self.heap.pace.state != St::Pause {
            self.singlestep_52();
        }
        before - self.heap.live_objects()
    }

    /// 5.2's step that finds no gray object: the atomic step, the
    /// estimate the memory traversed, and the sweep entered.
    pub(super) fn atomic_52(&mut self) -> i64 {
        let s = self.heap.sizes;
        let (work, swept) = self.gc_atomic();
        self.shrink_swept_52();
        // `GCestimate`: what the cycle traversed, `atomic` included
        self.heap.pace.estimate = self.heap.pace.trav + work;
        self.heap.pace.state = St::SweepString;
        self.heap.pace.strt_swept = 0;
        work + swept * sweepcost_52(&s)
    }

    pub(super) fn singlestep_52(&mut self) -> i64 {
        let s = self.heap.sizes;
        let (max, cost) = (sweepmax_52(&s), sweepcost_52(&s));
        match self.heap.pace.state {
            St::Pause => {
                self.gc_restart();
                let w = (self.heap.puc.strt_size * s.ptr) as i64;
                self.heap.pace.trav = w;
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
                Some(_) => {
                    self.shrink_swept_52();
                    max * cost
                }
                None => {
                    self.shrink_swept_52();
                    self.sweep_main_52();
                    self.heap.acct_check_sizes();
                    self.heap.pace.state = St::Pause;
                    cost
                }
            },
        }
    }
}
