//! PUC's incremental steps (`luaC_step` and `singlestep`), by dialect: the
//! states a cycle goes through, the work each step counts, and the debt a
//! step leaves. The marking and sweeping are luna's own (`heap::collect`,
//! `heap::sweep`); what is counted is PUC's.

use super::*;
use crate::runtime::heap::{Propagated, Running};
use crate::runtime::puc::pace::*;
use crate::runtime::table::Dialect;
use crate::vm::lib_gc::Param;

mod early;
mod pause;
mod phases;
mod recent;
mod step52;
use phases::sweep_other;

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
        let r = match d {
            Dialect::L51 => self.step_51(),
            Dialect::L52 => self.step_52(),
            Dialect::L53 => self.step_53(),
            Dialect::L54 => self.step_54(),
            Dialect::L55 => self.step_55(),
        };
        self.heap.refresh_gc_limit();
        // a 5.1–5.3 finalizer's error is thrown out of the step
        r?;
        #[cfg(feature = "gc-trace")]
        self.trace_step(true);
        Ok(())
    }

    /// PUC `luaC_step` with the collector not running (stopped, or a
    /// finalizer running): the debt put back so that it is not called
    /// again soon; 5.5 does so only for a collector the program stopped.
    fn gc_step_not_running(&mut self) {
        #[cfg(feature = "gc-trace")]
        self.trace_step(false);
        let d = self.heap.table_dialect;
        let debt = match d {
            Dialect::L52 => Some(-stepsize_52(&self.heap.puc.sizes)),
            Dialect::L53 => Some(-stepsize_52(&self.heap.puc.sizes) * 10),
            Dialect::L54 => Some(-2000),
            _ => self.heap.gc_is_stopped().then_some(20000),
        };
        if let Some(debt) = debt {
            let bytes = self.heap.bytes();
            self.heap.puc.pace.set_debt(d, bytes, debt);
            self.heap.refresh_gc_limit();
        }
        #[cfg(feature = "gc-trace")]
        self.trace_step(true);
    }

    /// A full collection's steps from its finalizers to its pause
    /// (`luaC_runtilstate(L, bitmask(GCSpause))`); the steps run whether
    /// or not the collector runs.
    pub(super) fn gc_steps_to_pause(&mut self) -> Result<(), LuaError> {
        let d = self.heap.table_dialect;
        self.heap.puc.pace.state = if d == Dialect::L51 {
            St::Finalize
        } else {
            St::CallFin
        };
        while self.heap.puc.pace.state != St::Pause {
            match d {
                Dialect::L51 => self.singlestep_51()?,
                Dialect::L53 => self.singlestep_53()?,
                Dialect::L54 => self.singlestep_54()?,
                _ => self.singlestep_55(true)?,
            };
        }
        Ok(())
    }

    /// The accounting trace's line for a step, as PUC's `luaC_step` logs
    /// it (`g` before, `G` after).
    #[cfg(feature = "gc-trace")]
    fn trace_step(&mut self, after: bool) {
        let d = self.heap.table_dialect;
        let c = if after { 'G' } else { 'g' };
        let bytes = self.heap.bytes();
        let p = self.heap.puc.pace;
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
        let debt_now = self.heap.puc.pace.debt(d, bytes);
        match d {
            Dialect::L51 => {
                self.heap.gc_unstop_51();
                let a = n.max(0) << 10;
                let t = if a <= bytes as i64 {
                    bytes as i64 - a
                } else {
                    0
                };
                self.heap.puc.pace.set_threshold(t);
                while self.heap.puc.pace.threshold() <= self.heap.bytes() as i64 {
                    self.puc_step_forced()?;
                    if self.heap.puc.pace.state == St::Pause {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Dialect::L52 => {
                let mut debt = n * 1024 - stepsize_52(&self.heap.puc.sizes);
                if !self.heap.gc_is_stopped() {
                    debt += debt_now;
                }
                self.heap.puc.pace.set_debt(d, bytes, debt);
                self.puc_step_forced()?;
                Ok(self.heap.puc.pace.state == St::Pause)
            }
            Dialect::L53 | Dialect::L54 => {
                let debt = if n == 0 {
                    let d0 = if d == Dialect::L53 {
                        -stepsize_52(&self.heap.puc.sizes)
                    } else {
                        0
                    };
                    self.heap.puc.pace.set_debt(d, bytes, d0);
                    self.puc_step_forced()?;
                    1
                } else {
                    let debt = n * 1024 + debt_now;
                    self.heap.puc.pace.set_debt(d, bytes, debt);
                    if debt > 0 {
                        self.puc_step_forced()?;
                    }
                    debt
                };
                Ok(debt > 0 && self.heap.puc.pace.state == St::Pause)
            }
            Dialect::L55 => {
                let n = if n <= 0 { debt_now } else { n };
                self.heap.puc.pace.set_debt(d, bytes, debt_now - n);
                let work = self.heap.puc.pace.debt(d, bytes) <= 0;
                if work {
                    self.puc_step_forced()?;
                }
                Ok(work && self.heap.puc.pace.state == St::Pause)
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
            let marked = self.heap.puc.pace.estimate;
            self.heap.trace.line(format_args!("x {marked}"));
        } else {
            self.heap.trace.line(format_args!("x"));
        }
    }
}
