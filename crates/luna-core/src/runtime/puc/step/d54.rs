//! The steps of 5.4.

use super::*;

pub(super) fn step_54<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let d = Dialect::L54;
    let s = h.heap().puc.sizes;
    let stepmul = (h.heap().puc.params.get(Param::StepMul)) | 1;
    let bytes = h.heap().bytes();
    let debt0 = h.heap().puc.pace.debt(d, bytes);
    let (mut debt, stepsize) =
        work_54(&s, debt0, stepmul, h.heap().puc.params.get(Param::StepSize));
    let run = h.running();
    loop {
        if h.heap().puc.pace.state() == St::Propagate {
            if h.propagate(&mut debt, -stepsize, run) {
                h.heap().puc.pace.enter(St::EnterAtomic);
            }
        } else {
            debt -= singlestep_54(h)?;
        }
        if !(debt > -stepsize && h.heap().puc.pace.state() != St::Pause) {
            break;
        }
    }
    let bytes = h.heap().bytes();
    if h.heap().puc.pace.state() == St::Pause {
        let pause = h.heap().puc.params.get(Param::Pause);
        let debt = pause_debt(d, bytes, h.heap().puc.pace.estimate, pause);
        h.heap().puc.pace.set_debt(d, bytes, debt);
    } else {
        let debt = debt_after_54(&s, debt, stepmul);
        h.heap().puc.pace.set_debt(d, bytes, debt);
    }
    Ok(())
}

pub(super) fn singlestep_54<H: StepHost + ?Sized>(h: &mut H) -> Result<i64, LuaError> {
    Ok(match h.heap().puc.pace.state() {
        St::Pause => {
            restart(h);
            1
        }
        St::Propagate => {
            h.heap().puc.pace.enter(St::EnterAtomic);
            0
        }
        St::EnterAtomic => {
            let (work, _) = h.atomic();
            h.heap().puc.pace.enter(St::SweepAll);
            h.heap().puc.pace.estimate = h.heap().bytes() as i64;
            work
        }
        St::SweepAll => match h.sweep_quota(SWEEPMAX_54) {
            Some((n, freed)) => {
                h.heap().puc.pace.estimate -= freed;
                n
            }
            None => {
                h.heap().puc.pace.enter(St::SweepFinobj);
                h.heap().puc.pace.fin_left = Some(h.heap().finobj_count());
                0
            }
        },
        St::SweepFinobj | St::SweepTobefnz => {
            let finobj = h.heap().puc.pace.state() == St::SweepFinobj;
            match h.heap().puc.pace.fin_left {
                Some(n) => {
                    sweep_other(&mut h.heap().puc.pace.fin_left, SWEEPMAX_54);
                    n.min(SWEEPMAX_54)
                }
                None => {
                    let next = if finobj {
                        h.heap().puc.pace.fin_left = Some(h.heap().tobefnz_count());
                        St::SweepTobefnz
                    } else {
                        St::SweepEnd
                    };
                    h.heap().puc.pace.enter(next);
                    0
                }
            }
        }
        St::SweepEnd => {
            h.heap().acct_check_sizes();
            h.heap().puc.pace.enter(St::CallFin);
            0
        }
        _ => {
            if h.heap().has_tobefnz() && h.finalizer_has_room(false) {
                let mut n = 0;
                while n < FINMAX_54 && h.heap().has_tobefnz() {
                    h.run_finalizer(false)?;
                    n += 1;
                }
                return Ok(n * FINALIZECOST_54);
            }
            h.heap().puc.pace.enter(St::Pause);
            0
        }
    })
}
