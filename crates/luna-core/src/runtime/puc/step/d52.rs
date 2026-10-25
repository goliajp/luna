//! The steps of 5.2.

use super::*;

pub(super) fn step_52<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let d = Dialect::L52;
    let s = h.heap().puc.sizes;
    let stepmul = h.heap().puc.params.get(Param::StepMul).max(40);
    let bytes = h.heap().bytes();
    let mut debt = work_52(d, h.heap().puc.pace.debt(d, bytes), stepmul);
    let floor = -stepsize_52(&s);
    let run = h.running();
    loop {
        if h.heap().puc.pace.state() == St::Propagate {
            let before = debt;
            let empty = h.propagate(&mut debt, floor, run);
            // what was traversed, for the estimate
            h.heap().puc.pace.trav += before - debt;
            if empty {
                debt -= atomic_52(h);
            }
        } else {
            debt -= singlestep_52(h);
        }
        if !(debt > floor && h.heap().puc.pace.state() != St::Pause) {
            break;
        }
    }
    let bytes = h.heap().bytes();
    if h.heap().puc.pace.state() == St::Pause {
        let pause = h.heap().puc.params.get(Param::Pause);
        let debt = pause_debt(d, bytes, h.heap().puc.pace.estimate, pause);
        h.heap().puc.pace.set_debt(d, bytes, debt);
    } else {
        h.heap()
            .puc
            .pace
            .set_debt(d, bytes, debt_after_52(debt, stepmul));
    }
    // `luaC_forcestep`: a few finalizers, all of them at a pause
    let mut i = 0;
    while h.heap().has_tobefnz() && (i < 4 || h.heap().puc.pace.state() == St::Pause) {
        h.run_finalizer(true)?;
        i += 1;
    }
    Ok(())
}

/// 5.2's `sweepthread` of each coroutine the sweep just kept (the main
/// thread's comes at the end of the sweep).
fn shrink_swept_52<H: StepHost + ?Sized>(h: &mut H) {
    let swept = std::mem::take(&mut h.heap().puc.shrunk);
    for &t in &swept {
        // SAFETY: the sweep just kept `t`, a coroutine
        h.shrink_swept_thread_52(unsafe { crate::runtime::Gc::from_ptr_unchecked(t.cast()) });
    }
    let mut swept = swept;
    swept.clear();
    h.heap().puc.shrunk = swept;
}

/// 5.2's `luaC_fullgc` from a pause: the whole cycle as single steps,
/// as PUC runs it, the estimate what it traverses. The objects freed.
/// `swept`: a cycle was under way, whose sweep has ended.
pub(crate) fn full_cycle_52<H: StepHost + ?Sized>(h: &mut H, swept: bool) -> usize {
    let before = h.heap().live_objects();
    if swept {
        shrink_swept_52(h);
        h.sweep_main_52();
    }
    h.heap().puc.pace.enter(St::Pause);
    singlestep_52(h);
    let run = h.running();
    let mut work = i64::MAX;
    while !h.propagate(&mut work, i64::MIN, run) {}
    h.heap().puc.pace.trav += i64::MAX - work;
    atomic_52(h);
    while h.heap().puc.pace.state() != St::Pause {
        singlestep_52(h);
    }
    before - h.heap().live_objects()
}

/// 5.2's step that finds no gray object: the atomic step, the
/// estimate the memory traversed, and the sweep entered.
pub(super) fn atomic_52<H: StepHost + ?Sized>(h: &mut H) -> i64 {
    let s = h.heap().puc.sizes;
    let (work, swept) = atomic(h);
    shrink_swept_52(h);
    // `GCestimate`: what the cycle traversed, `atomic` included
    h.heap().puc.pace.estimate = h.heap().puc.pace.trav + work;
    h.heap().puc.pace.enter(St::SweepString);
    h.heap().puc.pace.strt_swept = 0;
    work + swept * sweepcost_52(&s)
}

/// 5.2's `luaC_changemode` into generational mode: the cycle under way
/// runs on to its propagation (`luaC_runtilstate`), and the estimate is
/// what is in use.
pub(crate) fn enter_generational_52<H: StepHost + ?Sized>(h: &mut H) {
    while h.heap().puc.pace.state() != St::Propagate {
        singlestep_52(h);
    }
    h.heap().puc.pace.estimate = h.heap().bytes() as i64;
}

/// 5.2's `luaC_changemode` back to incremental mode: `entersweep`, then
/// the sweep to its end (`luaC_runtilstate(L, ~sweepphases)`).
pub(crate) fn leave_generational_52<H: StepHost + ?Sized>(h: &mut H) {
    if h.heap().gc_phase_is_pause() || h.heap().puc.pace.state() == St::Propagate {
        h.heap().gc_sweep_unmarked_52();
        h.heap().puc.pace.enter(St::SweepString);
        h.heap().puc.pace.strt_swept = 0;
    }
    while h.heap().puc.pace.state() != St::Pause {
        singlestep_52(h);
    }
}

pub(super) fn singlestep_52<H: StepHost + ?Sized>(h: &mut H) -> i64 {
    let s = h.heap().puc.sizes;
    let (max, cost) = (sweepmax_52(&s), sweepcost_52(&s));
    match h.heap().puc.pace.state() {
        St::Pause => {
            restart(h);
            let w = h.heap().strt_bytes() as i64;
            h.heap().puc.pace.trav = w;
            w
        }
        St::Propagate => atomic_52(h),
        St::SweepString => {
            let size = h.heap().puc.strt_size as i64;
            let i = max.min(size - h.heap().puc.pace.strt_swept);
            let heap = h.heap();
            heap.puc.pace.strt_swept += i;
            heap.sweep_strings_upto(heap.puc.pace.strt_swept);
            if h.heap().puc.pace.strt_swept >= size {
                strings_swept_52(h.heap());
            }
            i * cost
        }
        St::SweepUdata => {
            if sweep_other(&mut h.heap().puc.pace.fin_left, max) {
                return max * cost;
            }
            h.heap().puc.pace.enter(St::SweepAll);
            0
        }
        _ => match h.sweep_quota(max) {
            Some(_) => {
                shrink_swept_52(h);
                max * cost
            }
            None => {
                shrink_swept_52(h);
                h.sweep_main_52();
                h.heap().acct_check_sizes();
                h.heap().puc.pace.enter(St::Pause);
                cost
            }
        },
    }
}

/// 5.2's sweep of the string table is over: the objects with finalizers
/// come next.
pub(crate) fn strings_swept_52(heap: &mut Heap) {
    heap.end_string_sweep();
    heap.puc.pace.enter(St::SweepUdata);
    // `entersweep`'s `sweeptolive` sweeps `finobj` up to its first live
    // object: with no more after it there is no list left, and the next
    // step moves on at no cost
    let n = heap.finobj_count();
    heap.puc.pace.fin_left = (n > 1).then_some(n - 1);
}

/// `luaS_resize` while the collector sweeps the string table: 5.2 runs
/// that sweep to its end first (`luaC_runtilstate`), counted in no
/// step; 5.1 does not resize (false).
pub(crate) fn strt_resize_in_sweep(heap: &mut Heap) -> bool {
    if heap.puc.pace.state() != St::SweepString {
        return true;
    }
    if heap.puc.dialect == Dialect::L51 {
        return false;
    }
    heap.puc.pace.strt_swept = heap.puc.strt_size as i64;
    heap.sweep_strings_upto(heap.puc.pace.strt_swept);
    strings_swept_52(heap);
    true
}
