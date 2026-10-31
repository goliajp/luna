//! The collector's steps the VM drives, on the VM as their
//! [`StepHost`]: the step that fell due, the full collection and the
//! pause after it, a change of mode, `collectgarbage("step")`, and running
//! the finalizers a cycle left.

use crate::runtime::dialect::Dialect;

use crate::runtime::puc::Param;
use crate::runtime::puc::for_heap::ShrinkAt;
use crate::runtime::puc::pace::*;
use crate::runtime::puc::step::*;
use crate::vm::LuaError;

/// PUC `luaC_step`: the step that fell due.
pub(crate) fn puc_step<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    step_traced(h, true)
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

/// A full collection, a marking in progress dropped first; `emergency`
/// with the young objects for one an allocation runs. Returns the number
/// of objects freed (counted for the program's only).
pub(crate) fn full_collect<H: StepHost + ?Sized>(h: &mut H, emergency: Option<Young>) -> usize {
    let rules = *h.heap().puc_rules();
    let program = emergency.is_none();
    // a sweep (the one in progress, or 5.1's from a pause, which it treats
    // as marking) ends before the full cycle, with `checkSizes`; none when
    // 5.1's collector is past its sweep, running finalizers
    let swept = !h.heap().gc_phase_is_pause()
        || (rules.full_from_pause_sweeps && h.heap().puc.pace.state() != St::Finalize);
    gc_hold_threads(h);
    h.before_step();
    // a marking under way is dropped, the objects back to white, and the
    // collection starts over (`entersweep`), the program's or an
    // emergency's alike
    if h.heap().gc_phase_is_propagate() {
        h.heap().gc_abandon_marking();
    }
    h.heap().gc_finish_sweep();
    let freed = if program {
        if swept {
            h.heap().acct_check_sizes();
        }
        if rules.shrink_at == ShrinkAt::Sweep {
            full_cycle_52(h, swept)
        } else {
            full_mark_and_sweep(h, None)
        }
    } else if let (ShrinkAt::Sweep, Some(young)) = (rules.shrink_at, emergency) {
        emergency_cycle_52(h, young);
        0
    } else {
        full_mark_and_sweep(h, emergency)
    };
    h.collection_ends();
    if !program && rules.shrink_at == ShrinkAt::Sweep {
        h.shrink_threads_emergency();
    }
    freed
}

/// The collector goes from mode `prev` to `new` ("incremental" or
/// "generational"): 5.4 and 5.5 enter generational mode with a full
/// marking and sweep (`entergen`), which runs no finalizer; 5.2 runs its
/// cycle on to the propagation; going back, 5.5 sweeps first.
pub(crate) fn change_mode<H: StepHost + ?Sized>(h: &mut H, prev: &str, new: &str) {
    let d = h.heap().puc.dialect;
    if new == "generational" && prev != new && d >= Dialect::L54 {
        h.heap().trace_line(format_args!("X"));
        if !h.finalizing() && !h.heap().no_gc() {
            h.clear_dead_stack();
            full_collect(h, None);
            h.heap().finalizers_due();
            gc_full_pause(h);
            gc_minor_debt(h);
        }
        h.heap().trace_line(format_args!("x"));
    }
    if prev != new && d == Dialect::L52 {
        match new {
            "generational" => enter_generational_52(h),
            _ => leave_generational_52(h),
        }
    }
    if new == "incremental" && prev == "generational" {
        gc_leave_generational(h);
    }
}

/// A full collection ended (`luaC_fullgc`): the cycle at its pause,
/// the estimate what is left (5.5: what was marked), and the debt
/// `setpause` sets.
pub(crate) fn gc_full_pause<H: StepHost + ?Sized>(h: &mut H) {
    let d = h.heap().puc.dialect;
    let bytes = h.heap().bytes() as i64;
    let p = &mut h.heap().puc.pace;
    p.enter(St::Pause);
    p.fin_left = None;
    if d == Dialect::L51 {
        p.dept = 0;
    }
    // 5.2 keeps what the cycle traversed
    if !matches!(d, Dialect::L52 | Dialect::L55) {
        p.estimate = bytes;
    }
    gc_full_repause(h);
}

/// The pause a full collection sets (`setpause`, 5.1 `setthreshold`)
/// from the estimate it left, with the count as it is now: after the
/// finalizers it called but 5.2's.
pub(crate) fn gc_full_repause<H: StepHost + ?Sized>(h: &mut H) {
    let d = h.heap().puc.dialect;
    let bytes = h.heap().bytes();
    let heap = h.heap();
    let pause = heap.puc.params.get(Param::Pause);
    let params = &heap.puc.params;
    let p = &mut heap.puc.pace;
    match d {
        Dialect::L51 => p.set_threshold(threshold_51(p.estimate, pause)),
        Dialect::L55 => {
            let threshold = params.apply(Param::Pause, p.estimate);
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
    h.heap().refresh_gc_limit();
}
