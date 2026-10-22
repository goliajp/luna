//! The steps of 5.5.

use super::*;

pub(super) fn step_55<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let d = Dialect::L55;
    let stepsize = h.heap().puc.params.apply(Param::StepSize, 100);
    let ptr = h.heap().puc.sizes.ptr as i64;
    let mut work = h.heap().puc.params.apply(Param::StepMul, stepsize / ptr);
    let fast = work == 0;
    let run = h.running();
    loop {
        if !fast && h.heap().puc.pace.state() == St::Propagate {
            if h.propagate(&mut work, 0, run) {
                // the step that finds no gray object
                h.heap().puc.pace.enter(St::EnterAtomic);
                work -= 1;
            }
        } else {
            let r = singlestep_55(h, fast)?;
            if r == STEP2PAUSE || (r == ATOMICSTEP && !fast) {
                break;
            }
            work -= r;
        }
        if !(fast || work > 0) {
            break;
        }
    }
    let bytes = h.heap().bytes();
    let debt = if h.heap().puc.pace.state() == St::Pause {
        let estimate = h.heap().puc.pace.estimate;
        let threshold = h.heap().puc.params.apply(Param::Pause, estimate);
        pause_debt_55(bytes, threshold)
    } else {
        stepsize
    };
    h.heap().puc.pace.set_debt(d, bytes, debt);
    Ok(())
}

pub(super) fn singlestep_55<H: StepHost + ?Sized>(h: &mut H, fast: bool) -> Result<i64, LuaError> {
    Ok(match h.heap().puc.pace.state() {
        St::Pause => {
            h.restart();
            1
        }
        St::Propagate => {
            if fast {
                h.heap().puc.pace.enter(St::EnterAtomic);
                return Ok(1);
            }
            let run = h.running();
            let mut w = 1;
            if h.propagate(&mut w, 0, run) {
                h.heap().puc.pace.enter(St::EnterAtomic);
                return Ok(1);
            }
            1 - w
        }
        St::EnterAtomic => {
            h.atomic();
            h.heap().puc.pace.enter(St::SweepAll);
            ATOMICSTEP
        }
        St::CallFin => {
            if h.heap().has_tobefnz() && h.finalizer_has_room(false) {
                h.run_finalizer(false)?;
                return Ok(CWUFIN_55);
            }
            h.heap().puc.pace.enter(St::Pause);
            STEP2PAUSE
        }
        _ => sweep_step_55(h, fast),
    })
}

pub(super) fn sweep_step_55<H: StepHost + ?Sized>(h: &mut H, fast: bool) -> i64 {
    let quota = if fast { i64::MAX } else { SWEEPMAX_55 };
    match h.heap().puc.pace.state() {
        St::SweepAll => {
            if h.sweep_quota(quota).is_none() {
                h.heap().puc.pace.enter(St::SweepFinobj);
                h.heap().puc.pace.fin_left = Some(h.heap().finobj_count());
            }
            SWEEPMAX_55
        }
        St::SweepFinobj => {
            if !sweep_other(&mut h.heap().puc.pace.fin_left, quota) {
                h.heap().puc.pace.enter(St::SweepTobefnz);
                h.heap().puc.pace.fin_left = Some(h.heap().tobefnz_count());
            }
            SWEEPMAX_55
        }
        St::SweepTobefnz => {
            if !sweep_other(&mut h.heap().puc.pace.fin_left, quota) {
                h.heap().puc.pace.enter(St::SweepEnd);
            }
            SWEEPMAX_55
        }
        St::SweepEnd => {
            h.heap().acct_check_sizes();
            h.heap().puc.pace.enter(St::CallFin);
            SWEEPMAX_55
        }
        _ => unreachable!("a sweep state"),
    }
}
