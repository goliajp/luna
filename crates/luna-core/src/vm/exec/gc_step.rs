//! PUC's incremental steps (`luaC_step` and `singlestep`), by dialect: the
//! states a cycle goes through, the work each step counts, and the debt a
//! step leaves. The marking and sweeping are luna's own (`heap_step.rs`,
//! `heap_sweep.rs`); what is counted is PUC's.

use super::*;
use crate::runtime::gc_pace::*;
use crate::runtime::heap::Running;
use crate::runtime::table::Dialect;
use crate::vm::lib_gc::Param;

mod early;
mod recent;

/// 5.5 `singlestep`'s results that end `incstep`'s loop.
const STEP2PAUSE: i64 = -3;
const ATOMICSTEP: i64 = -2;

impl Vm {
    /// PUC `luaC_step`: the step that fell due.
    pub(crate) fn puc_step(&mut self) -> Result<(), LuaError> {
        let d = self.heap.table_dialect;
        if d != Dialect::L51 && !self.heap.gc_steps_run() {
            self.gc_step_not_running();
            return Ok(());
        }
        self.gc_hold_threads();
        self.jit_forget_ghosts();
        self.name_cache = Default::default();
        #[cfg(feature = "gc-trace")]
        self.trace_step(false);
        match d {
            Dialect::L51 => self.step_51(),
            Dialect::L52 => self.step_52(),
            Dialect::L53 => self.step_53(),
            Dialect::L54 => self.step_54(),
            Dialect::L55 => self.step_55(),
        }
        #[cfg(feature = "gc-trace")]
        self.trace_step(true);
        self.heap.refresh_gc_limit();
        // a finalizer's error a 5.1–5.3 step throws out of the check
        match self.gc_raise.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// PUC `luaC_step` with the collector not running (stopped, or a
    /// finalizer running): the debt put back so that it is not called
    /// again soon; 5.5 does so only for a collector the program stopped.
    fn gc_step_not_running(&mut self) {
        #[cfg(feature = "gc-trace")]
        self.trace_step(false);
        let d = self.heap.table_dialect;
        let debt = match d {
            Dialect::L52 => Some(-stepsize_52(&self.heap.sizes)),
            Dialect::L53 => Some(-stepsize_52(&self.heap.sizes) * 10),
            Dialect::L54 => Some(-2000),
            _ => self.heap.gc_is_stopped().then_some(20000),
        };
        if let Some(debt) = debt {
            let bytes = self.heap.bytes();
            self.heap.pace.set_debt(d, bytes, debt);
            self.heap.refresh_gc_limit();
        }
        #[cfg(feature = "gc-trace")]
        self.trace_step(true);
    }

    /// A full collection ended (`luaC_fullgc`): the cycle at its pause,
    /// the estimate what is left (5.5: what was marked), and the debt
    /// `setpause` sets.
    pub(crate) fn gc_full_pause(&mut self) {
        let d = self.heap.table_dialect;
        let bytes = self.heap.bytes();
        let pause = self.gc_params.get(Param::Pause);
        let p = &mut self.heap.pace;
        p.state = St::Pause;
        p.fin_left = None;
        match d {
            Dialect::L51 => {
                p.estimate = bytes as i64;
                p.dept = 0;
                p.set_threshold(threshold_51(bytes as i64, pause));
            }
            Dialect::L55 => {
                let threshold = self.gc_params.apply(Param::Pause, p.estimate);
                self.heap
                    .pace
                    .set_debt(d, bytes, pause_debt_55(bytes, threshold));
            }
            _ => {
                p.estimate = bytes as i64;
                p.set_debt(d, bytes, pause_debt(d, bytes, bytes as i64, pause));
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
                .apply(Param::MinorMul, self.heap.pace.estimate),
            _ => return,
        };
        self.heap.pace.set_debt(d, bytes, debt);
        if d == Dialect::L55 {
            // `GCmarked` counts the bytes that become old from here
            self.heap.pace.estimate = 0;
        }
        self.heap.refresh_gc_limit();
    }

    /// Back to incremental mode: 5.4 waits for its next cycle (`enterinc`);
    /// 5.5 sweeps first, a step's size away (`minor2inc`).
    pub(crate) fn gc_leave_generational(&mut self) {
        if self.heap.table_dialect != Dialect::L55 || !self.heap.gc_phase_is_pause() {
            return;
        }
        self.heap.pace.count_marked = false;
        self.heap.gc_sweep_all_live();
        self.heap.pace.state = St::SweepAll;
        let bytes = self.heap.bytes();
        let debt = self.gc_params.apply(Param::StepSize, 100);
        self.heap.pace.set_debt(Dialect::L55, bytes, debt);
        self.heap.refresh_gc_limit();
    }

    /// The accounting trace's line for a step, as PUC's `luaC_step` logs
    /// it (`g` before, `G` after).
    #[cfg(feature = "gc-trace")]
    fn trace_step(&mut self, after: bool) {
        let d = self.heap.table_dialect;
        let c = if after { 'G' } else { 'g' };
        let bytes = self.heap.bytes();
        let p = self.heap.pace;
        let st = p.state.number(d);
        let kind = i32::from(self.gc_mode_is_generational());
        match d {
            Dialect::L51 => self.heap.trace.line(format_args!(
                "{c} {st} {bytes} {} {} {}",
                p.threshold(),
                p.estimate,
                p.dept
            )),
            _ => self.heap.trace.line(format_args!(
                "{c} {st} {bytes} {} {} {kind}",
                p.debt(d, bytes),
                p.estimate
            )),
        }
    }

    /// `lua_gc(L, LUA_GCSTEP, n)` (`collectgarbage("step", n)`) in
    /// incremental mode: the debt it sets and the steps it runs, whether
    /// or not the collector is stopped; true when a cycle ended.
    pub(crate) fn gc_lua_step(&mut self, n: i64) -> Result<bool, LuaError> {
        // the instrumented PUC that `R38_NOAUTO` compares with skips every
        // `luaC_step`, the ones `lua_gc` asks for too
        if self.heap.no_auto() {
            return Ok(false);
        }
        let d = self.heap.table_dialect;
        let bytes = self.heap.bytes();
        let debt_now = self.heap.pace.debt(d, bytes);
        match d {
            Dialect::L51 => {
                let a = n.max(0) << 10;
                let t = if a <= bytes as i64 {
                    bytes as i64 - a
                } else {
                    0
                };
                self.heap.pace.set_threshold(t);
                while self.heap.pace.threshold() <= self.heap.bytes() as i64 {
                    self.puc_step_forced()?;
                    if self.heap.pace.state == St::Pause {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Dialect::L52 => {
                let mut debt = n * 1024 - stepsize_52(&self.heap.sizes);
                if !self.heap.gc_is_stopped() {
                    debt += debt_now;
                }
                self.heap.pace.set_debt(d, bytes, debt);
                self.puc_step_forced()?;
                Ok(self.heap.pace.state == St::Pause)
            }
            Dialect::L53 | Dialect::L54 => {
                let debt = if n == 0 {
                    let d0 = if d == Dialect::L53 {
                        -stepsize_52(&self.heap.sizes)
                    } else {
                        0
                    };
                    self.heap.pace.set_debt(d, bytes, d0);
                    self.puc_step_forced()?;
                    1
                } else {
                    let debt = n * 1024 + debt_now;
                    self.heap.pace.set_debt(d, bytes, debt);
                    if debt > 0 {
                        self.puc_step_forced()?;
                    }
                    debt
                };
                Ok(debt > 0 && self.heap.pace.state == St::Pause)
            }
            Dialect::L55 => {
                let n = if n <= 0 { debt_now } else { n };
                self.heap.pace.set_debt(d, bytes, debt_now - n);
                let work = self.heap.pace.debt(d, bytes) <= 0;
                if work {
                    self.puc_step_forced()?;
                }
                Ok(work && self.heap.pace.state == St::Pause)
            }
        }
    }

    /// A step `lua_gc` asks for runs with the collector running: 5.2's
    /// `luaC_forcestep` whatever it is; 5.3 on, `gcrunning` (`gcstp`) set
    /// for it and put back after, unless the step raised.
    fn puc_step_forced(&mut self) -> Result<(), LuaError> {
        if self.heap.table_dialect == Dialect::L51 {
            return self.puc_step();
        }
        let (stopped, fin_stop) = (self.heap.gc_is_stopped(), self.heap.gc_fin_stop);
        self.heap.gc_fin_stop = false;
        self.heap.gc_restore_stopped(false);
        let r = self.puc_step();
        if r.is_ok() || self.heap.table_dialect == Dialect::L52 {
            self.heap.gc_fin_stop = fin_stop;
            self.heap.gc_restore_stopped(stopped);
        }
        r
    }
    /// The end of a full collection in the trace: 5.5 with its `GCmarked`.
    #[cfg(feature = "gc-trace")]
    pub(crate) fn trace_full_end(&mut self) {
        if self.heap.table_dialect == Dialect::L55 {
            let marked = self.heap.pace.estimate;
            self.heap.trace.line(format_args!("x {marked}"));
        } else {
            self.heap.trace.line(format_args!("x"));
        }
    }

    /// The running thread, whose stack the VM holds.
    fn gc_running(&self) -> Running {
        let co = self.current.or(self.main_coro);
        let main = match (&self.main_ctx, self.main_coro) {
            (Some(m), Some(mc)) => Some((mc.as_ptr() as *mut _, m.stack_size, m.ci_alloc)),
            _ => None,
        };
        let (main, main_size, main_ci) = main.unwrap_or((std::ptr::null_mut(), 0, 0));
        Running {
            h: co.map_or(std::ptr::null_mut(), |c| c.as_ptr() as *mut _),
            top: self.gc_top,
            size: self.stack_size,
            ci: if self.heap.table_dialect == Dialect::L51 {
                self.g.frame_size
            } else {
                self.g.ci_alloc
            },
            main,
            main_size,
            main_ci,
        }
    }

    /// The threads as the collector is about to find them.
    pub(crate) fn gc_hold_threads(&mut self) {
        self.heap.pace.run = Some(self.gc_running());
    }

    /// A marking starts from the roots: 5.5's `GCmarked` counts from 0
    /// what it marks (`restartcollection`).
    pub(crate) fn gc_count_marked(&mut self) {
        let count = self.heap.table_dialect == Dialect::L55;
        self.heap.pace.count_marked = count;
        if count {
            self.heap.pace.estimate = 0;
        }
    }

    /// `restartcollection` (5.1 `markroot`): the roots marked, the cycle
    /// propagating.
    fn gc_restart(&mut self) {
        self.gc_count_marked();
        let mut m = self.heap.gc_begin_propagate();
        self.mark_roots(&mut m, false);
        self.heap.stash_marker(m);
        self.heap.pace.state = St::Propagate;
    }

    /// PUC `atomic` and `entersweep`; returns the work and how many
    /// objects the sweep went over on the way in.
    fn gc_atomic(&mut self) -> (i64, i64) {
        self.clear_dead_stack();
        let run = self.gc_running();
        let mut m = self.heap.loan_marker();
        self.mark_roots(&mut m, true);
        self.heap.stash_marker(m);
        self.watch_errors();
        let r = self.heap.gc_finish_atomic_work(Some(run));
        self.collector_shrinks_threads();
        self.forget_dead_errors();
        // luna runs the finalizers the cycle found at once
        self.run_finalizers();
        r
    }

    /// The list sweep of a state that sweeps `quota` objects: how many
    /// it swept and the bytes it freed; `None` once the list is done.
    fn sweep_quota(&mut self, quota: i64) -> Option<(i64, i64)> {
        if !self.heap.gc_phase_is_sweep() {
            return None;
        }
        let before = self.heap.bytes() as i64;
        let (n, _) = self.heap.sweep_counted(quota);
        Some((n, before - self.heap.bytes() as i64))
    }
}

/// A sweep over one of PUC's other lists (the objects with finalizers,
/// the ones to be finalized), which luna keeps on its one list: `left`
/// goes down by `quota`; `None` once it is done.
fn sweep_other(left: &mut Option<i64>, quota: i64) -> bool {
    match left {
        Some(n) => {
            *n -= quota;
            if *n <= 0 {
                *left = None;
            }
            true
        }
        None => false,
    }
}
