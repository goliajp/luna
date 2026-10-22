//! The steps of 5.3.

use super::*;

pub(super) fn step_53<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let d = Dialect::L53;
    let s = h.heap().puc.sizes;
    let stepmul = h.heap().puc.params.get(Param::StepMul);
    let bytes = h.heap().bytes();
    let mut debt = work_52(d, h.heap().puc.pace.debt(d, bytes), stepmul);
    let floor = -stepsize_52(&s);
    let run = h.running();
    loop {
        if h.heap().puc.pace.state() == St::Propagate {
            h.propagate(&mut debt, floor, run);
            if h.heap().gray_is_empty() {
                h.heap().puc.pace.enter(St::Atomic);
            }
        } else {
            debt -= singlestep_53(h)?;
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
        run_a_few_finalizers(h)?;
    }
    Ok(())
}

/// 5.3 `runafewfinalizers`: `gcfinnum` of them, twice as many the
/// next time while some are left.
pub(super) fn run_a_few_finalizers<H: StepHost + ?Sized>(h: &mut H) -> Result<i64, LuaError> {
    let mut i = 0;
    while h.heap().has_tobefnz() && i < h.heap().puc.pace.fin_num {
        h.run_finalizer(true)?;
        i += 1;
    }
    h.heap().puc.pace.fin_num = if h.heap().has_tobefnz() {
        h.heap().puc.pace.fin_num * 2
    } else {
        0
    };
    Ok(i64::from(i))
}

pub(super) fn singlestep_53<H: StepHost + ?Sized>(h: &mut H) -> Result<i64, LuaError> {
    let s = h.heap().puc.sizes;
    let cost = sweepmax_52(&s) * sweepcost_52(&s);
    Ok(match h.heap().puc.pace.state() {
        St::Pause => {
            restart(h);
            (h.heap().puc.strt_size * s.ptr) as i64
        }
        St::Propagate => {
            h.heap().puc.pace.enter(St::Atomic);
            0
        }
        St::Atomic => {
            let (work, _) = h.atomic();
            h.heap().puc.pace.enter(St::SweepAll);
            h.heap().puc.pace.estimate = h.heap().bytes() as i64;
            work
        }
        St::SweepAll => match h.sweep_quota(sweepmax_52(&s)) {
            Some((_, freed)) if h.heap().gc_phase_is_sweep() => {
                h.heap().puc.pace.estimate -= freed;
                cost
            }
            r => {
                if let Some((_, freed)) = r {
                    h.heap().puc.pace.estimate -= freed;
                }
                h.heap().puc.pace.enter(St::SweepFinobj);
                h.heap().puc.pace.fin_left = Some(h.heap().finobj_count());
                0
            }
        },
        St::SweepFinobj | St::SweepTobefnz => {
            let finobj = h.heap().puc.pace.state() == St::SweepFinobj;
            if sweep_other(&mut h.heap().puc.pace.fin_left, sweepmax_52(&s))
                && h.heap().puc.pace.fin_left.is_some()
            {
                return Ok(cost);
            }
            let next = if finobj {
                h.heap().puc.pace.fin_left = Some(h.heap().tobefnz_count());
                St::SweepTobefnz
            } else {
                St::SweepEnd
            };
            h.heap().puc.pace.enter(next);
            0
        }
        St::SweepEnd => {
            h.heap().acct_check_sizes();
            h.heap().puc.pace.enter(St::CallFin);
            0
        }
        _ => {
            // emergency collections run no finalizer (they are full
            // ones, which never step here)
            if h.heap().has_tobefnz() {
                let n = run_a_few_finalizers(h)?;
                return Ok(n * sweepcost_52(&s));
            }
            h.heap().puc.pace.enter(St::Pause);
            0
        }
    })
}
