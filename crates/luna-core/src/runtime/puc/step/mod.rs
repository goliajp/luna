//! PUC's incremental steps (`luaC_step` and `singlestep`), by dialect: the
//! states a cycle goes through, the work each step counts, and the debt a
//! step leaves. What only the VM can do (mark the roots, run a finalizer,
//! shrink a stack) it does for them through [`StepHost`]; the formulas and
//! the state are the model's.

use crate::runtime::dialect::Dialect;
use crate::runtime::heap::{Heap, Marker, Running};
use crate::runtime::puc::Param;
use crate::runtime::puc::pace::*;
use crate::vm::LuaError;

mod atomic;
mod d51;
mod d52;
mod d53;
mod d54;
mod d55;
mod full;
mod host;
mod mode;
mod pause;
pub(super) use atomic::atomic;
use d51::*;
use d52::*;
pub(crate) use d52::{
    enter_generational_52, full_cycle_52, leave_generational_52, strt_resize_in_sweep,
};
use d53::*;
use d54::*;
use d55::*;
pub(crate) use full::full_collect;
pub(crate) use host::{CiCount, StepHost};
pub(crate) use mode::change_mode;
pub(crate) use pause::{gc_full_pause, gc_full_repause, gc_leave_generational, gc_minor_debt};

/// 5.5 `singlestep`'s results that end `incstep`'s loop.
const STEP2PAUSE: i64 = -3;
const ATOMICSTEP: i64 = -2;

/// `restartcollection` (5.1 `markroot`): the roots marked, the cycle
/// propagating.
fn restart<H: StepHost + ?Sized>(h: &mut H) {
    // no thread shrunk yet this cycle
    h.heap().puc.shrunk.clear();
    gc_count_marked(h);
    let mut m = h.heap().gc_begin_propagate();
    h.mark_roots_restart(&mut m);
    if h.heap().puc.rules.tobefnz_at_restart {
        h.heap().mark_tobefnz(&mut m);
    }
    h.heap().stash_marker(m);
    h.heap().puc.pace.enter(St::Propagate);
}

/// PUC `luaC_step`: the step that fell due.
pub(crate) fn puc_step<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let d = h.heap().puc.dialect;
    if d != Dialect::L51 && !h.heap().gc_steps_run() {
        gc_step_not_running(h);
        return Ok(());
    }
    gc_hold_threads(h);
    h.before_step();
    crate::runtime::gc_hooks::hook_step(h, false);
    let r = match d {
        Dialect::L51 => step_51(h),
        Dialect::L52 => step_52(h),
        Dialect::L53 => step_53(h),
        Dialect::L54 => step_54(h),
        Dialect::L55 => step_55(h),
    };
    h.heap().refresh_gc_limit();
    // a 5.1–5.3 finalizer's error is thrown out of the step
    r?;
    crate::runtime::gc_hooks::hook_step(h, true);
    Ok(())
}

/// PUC `luaC_step` with the collector not running (stopped, or a
/// finalizer running): the debt put back so that it is not called
/// again soon; 5.5 does so only for a collector the program stopped.
pub(super) fn gc_step_not_running<H: StepHost + ?Sized>(h: &mut H) {
    crate::runtime::gc_hooks::hook_step(h, false);
    let d = h.heap().puc.dialect;
    let debt = match d {
        Dialect::L52 => Some(-stepsize_52(&h.heap().puc.sizes)),
        Dialect::L53 => Some(-stepsize_52(&h.heap().puc.sizes) * 10),
        Dialect::L54 => Some(-2000),
        _ => h.heap().gc_is_stopped().then_some(20000),
    };
    if let Some(debt) = debt {
        let bytes = h.heap().bytes();
        h.heap().puc.pace.set_debt(d, bytes, debt);
        h.heap().refresh_gc_limit();
    }
    crate::runtime::gc_hooks::hook_step(h, true);
}

/// A full collection's steps from its finalizers to its pause
/// (`luaC_runtilstate(L, bitmask(GCSpause))`); the steps run whether
/// or not the collector runs.
pub(crate) fn gc_steps_to_pause<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let d = h.heap().puc.dialect;
    h.heap().puc.pace.enter(if d == Dialect::L51 {
        St::Finalize
    } else {
        St::CallFin
    });
    while h.heap().puc.pace.state() != St::Pause {
        match d {
            Dialect::L51 => singlestep_51(h)?,
            Dialect::L53 => singlestep_53(h)?,
            Dialect::L54 => singlestep_54(h)?,
            _ => singlestep_55(h, true)?,
        };
    }
    Ok(())
}

/// 5.3 on, a full collection first runs the finalizers left from a cycle
/// whose finalizer raised, to its pause (`luaC_runtilstate(L,
/// bitmask(GCSpause))` from `GCScallfin`), before its own cycle.
pub(crate) fn gc_finish_finalizers<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    if h.heap().puc.pace.state() == St::CallFin {
        gc_steps_to_pause(h)?;
    }
    Ok(())
}

/// The instrumented PUC that `LUNA_GC_NOAUTO` compares with skips every
/// `luaC_step`, the ones `lua_gc` asks for too; 5.2's `lua_gc` runs
/// `luaC_forcestep`, which it does not skip.
pub(crate) fn lua_step_skipped(heap: &crate::runtime::heap::Heap) -> bool {
    heap.no_auto() && heap.puc.dialect != Dialect::L52
}

/// `lua_gc(L, LUA_GCSTEP, n)` (`collectgarbage("step", n)`) in
/// incremental mode: the debt it sets and the steps it runs, whether
/// or not the collector is stopped; true when a cycle ended.
pub(crate) fn gc_lua_step<H: StepHost + ?Sized>(h: &mut H, n: i64) -> Result<bool, LuaError> {
    if lua_step_skipped(h.heap()) {
        return Ok(false);
    }
    let d = h.heap().puc.dialect;
    let bytes = h.heap().bytes();
    let debt_now = h.heap().puc.pace.debt(d, bytes);
    match d {
        Dialect::L51 => {
            h.heap().gc_unstop_51();
            let a = n.max(0) << 10;
            let t = if a <= bytes as i64 {
                bytes as i64 - a
            } else {
                0
            };
            h.heap().puc.pace.set_threshold(t);
            while h.heap().puc.pace.threshold() <= h.heap().bytes() as i64 {
                puc_step_forced(h)?;
                if h.heap().puc.pace.state() == St::Pause {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Dialect::L52 => {
            let mut debt = n * 1024 - stepsize_52(&h.heap().puc.sizes);
            if !h.heap().gc_is_stopped() {
                debt += debt_now;
            }
            h.heap().puc.pace.set_debt(d, bytes, debt);
            puc_step_forced(h)?;
            Ok(h.heap().puc.pace.state() == St::Pause)
        }
        Dialect::L53 | Dialect::L54 => {
            let debt = if n == 0 {
                let d0 = if d == Dialect::L53 {
                    -stepsize_52(&h.heap().puc.sizes)
                } else {
                    0
                };
                h.heap().puc.pace.set_debt(d, bytes, d0);
                puc_step_forced(h)?;
                1
            } else {
                let debt = n * 1024 + debt_now;
                h.heap().puc.pace.set_debt(d, bytes, debt);
                if debt > 0 {
                    puc_step_forced(h)?;
                }
                debt
            };
            Ok(debt > 0 && h.heap().puc.pace.state() == St::Pause)
        }
        Dialect::L55 => {
            let n = if n <= 0 { debt_now } else { n };
            h.heap().puc.pace.set_debt(d, bytes, debt_now - n);
            let work = h.heap().puc.pace.debt(d, bytes) <= 0;
            if work {
                puc_step_forced(h)?;
            }
            Ok(work && h.heap().puc.pace.state() == St::Pause)
        }
    }
}

/// A step `lua_gc` asks for runs with the collector running: 5.2's
/// `luaC_forcestep` whatever it is; 5.3 on, `gcrunning` (`gcstp`) set
/// for it and put back after, unless the step raised.
pub(super) fn puc_step_forced<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    if h.heap().puc.dialect == Dialect::L51 {
        return puc_step(h);
    }
    let run = h.heap().gc_run_mark();
    h.heap().gc_set_fin_stop(false);
    h.heap().gc_restore_stopped(false);
    let r = puc_step(h);
    if r.is_ok() || h.heap().puc.dialect == Dialect::L52 {
        h.heap().gc_restore_run(run);
    }
    r
}

/// The threads as the collector is about to find them.
pub(crate) fn gc_hold_threads<H: StepHost + ?Sized>(h: &mut H) {
    h.heap().puc.pace.run = Some(h.running());
}

/// A marking starts from the roots: 5.5's `GCmarked` counts from 0
/// what it marks (`restartcollection`).
pub(crate) fn gc_count_marked<H: StepHost + ?Sized>(h: &mut H) {
    let count = h.heap().puc.dialect == Dialect::L55;
    h.heap().puc.pace.count_marked = count;
    if count {
        h.heap().puc.pace.estimate = 0;
    }
}

/// A sweep over one of PUC's other lists (the objects with finalizers,
/// the ones to be finalized), which luna keeps on its one list: `left`
/// goes down by `quota`; `None` once it is done.
pub(super) fn sweep_other(left: &mut Option<i64>, quota: i64) -> bool {
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
