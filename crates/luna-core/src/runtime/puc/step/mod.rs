//! PUC's incremental steps (`luaC_step` and `singlestep`), by dialect: the
//! states a cycle goes through, the work each step counts, and the debt a
//! step leaves. What only the VM can do (mark the roots, run a finalizer,
//! shrink a stack) it does for them through [`StepHost`]; the formulas and
//! the state are the model's.

use crate::runtime::dialect::Dialect;
use crate::runtime::heap::{Heap, Marker, Running};
use crate::runtime::puc::Param;
use crate::runtime::puc::pace::*;

mod atomic;
mod d51;
mod d52;
mod d53;
mod d54;
mod d55;
mod full;
mod host;
mod pause;
#[cfg(test)]
mod tests;
pub(super) use atomic::atomic;
use d51::*;
pub(in crate::runtime::puc) use d52::strt_resize_in_sweep;
use d52::*;
use d53::*;
use d54::*;
use d55::*;

pub(in crate::runtime::puc) use d51::singlestep_51;
pub(in crate::runtime::puc) use d52::{
    emergency_cycle_52, enter_generational_52, full_cycle_52, leave_generational_52,
};
pub(in crate::runtime::puc) use d53::singlestep_53;
pub(in crate::runtime::puc) use d54::singlestep_54;
pub(in crate::runtime::puc) use d55::singlestep_55;
pub(in crate::runtime::puc) use full::{Young, full_mark_and_sweep};
pub(crate) use host::{CiCount, StepHost};

pub(in crate::runtime::puc) use pause::{gc_leave_generational, gc_minor_debt};

/// 5.5 `singlestep`'s results that end `incstep`'s loop.
const STEP2PAUSE: i64 = -3;
const ATOMICSTEP: i64 = -2;

/// `restartcollection` (5.1 `markroot`): the roots marked, the cycle
/// propagating.
fn restart<H: StepHost + ?Sized>(h: &mut H) {
    // 5.2: the threads a sweep kept are shrunk before a program's full
    // collection runs its cycle (`full_cycle_52`) and dropped before an
    // emergency's (`emergency_cycle_52`), whose sweep shrinks none
    if crate::runtime::gc_hooks::DEBUG_CHECKS {
        assert!(
            h.heap().puc.records.swept_52.is_empty(),
            "5.2: threads a sweep kept are left unshrunk at the next cycle"
        );
    }
    let c = h.heap().puc.cycle;
    h.heap().puc.cycle = c.checked_add(1).unwrap_or(std::num::NonZeroU32::MIN);
    gc_count_marked(h);
    let mut m = h.heap().gc_begin_propagate();
    h.mark_roots_restart(&mut m);
    if h.heap().puc.rules.tobefnz_at_restart {
        h.heap().mark_tobefnz(&mut m);
    }
    h.heap().stash_marker(m);
    h.heap().puc.pace.enter(St::Propagate);
}

/// [`puc_step`]; `traced`: gc-trace logs it as a `luaC_step` (5.2's
/// `lua_gc` calls `luaC_forcestep`, which the trace does not log).
pub(in crate::runtime::puc) fn step_traced<H: StepHost + ?Sized>(
    h: &mut H,
    traced: bool,
) -> Result<(), H::Error> {
    let d = h.heap().puc.dialect;
    if d != Dialect::L51 && !h.heap().gc_steps_run() {
        gc_step_not_running(h);
        return Ok(());
    }
    gc_hold_threads(h);
    h.before_step();
    if traced {
        crate::runtime::gc_hooks::hook_step(h, false);
    }
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
    if traced {
        crate::runtime::gc_hooks::hook_step(h, true);
    }
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

/// A step `lua_gc` asks for runs with the collector running: 5.2's
/// `luaC_forcestep` whatever it is; 5.3 on, `gcrunning` (`gcstp`) set
/// for it and put back after, unless the step raised.
pub(super) fn puc_step_forced<H: StepHost + ?Sized>(h: &mut H) -> Result<(), H::Error> {
    if h.heap().puc.dialect == Dialect::L51 {
        return super::api::puc_step(h);
    }
    let run = h.heap().gc_run_mark();
    h.heap().gc_set_fin_stop(false);
    h.heap().gc_restore_stopped(false);
    let traced = h.heap().puc.dialect != Dialect::L52;
    let r = step_traced(h, traced);
    if r.is_ok() || h.heap().puc.dialect == Dialect::L52 {
        h.heap().gc_restore_run(run);
    }
    r
}

/// The threads as the collector is about to find them.
pub(super) fn gc_hold_threads<H: StepHost + ?Sized>(h: &mut H) {
    h.heap().puc.pace.run = Some(h.running());
}

/// A marking starts from the roots: 5.5's `GCmarked` counts from 0
/// what it marks (`restartcollection`).
pub(super) fn gc_count_marked<H: StepHost + ?Sized>(h: &mut H) {
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
