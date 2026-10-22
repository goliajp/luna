//! The steps of 5.1.

use super::*;

pub(super) fn step_51<H: StepHost + ?Sized>(h: &mut H) -> Result<(), LuaError> {
    let mut lim = step_limit_51(h.heap().puc.params.get(Param::StepMul));
    let bytes = h.heap().bytes() as i64;
    let p = &mut h.heap().puc.pace;
    p.dept += bytes - p.threshold();
    let run = h.running();
    loop {
        if h.heap().puc.pace.state() == St::Propagate {
            if h.propagate(&mut lim, 0, run) {
                atomic_51(h);
            }
        } else if h.heap().puc.pace.state() == St::SweepString {
            // the buckets the single steps would sweep before the limit,
            // at once: nothing else happens between them
            let heap = h.heap();
            let left = heap.puc.strt_size as i64 - heap.puc.pace.strt_swept;
            let k = ((lim + SWEEPCOST_51 - 1) / SWEEPCOST_51).clamp(1, left.max(1));
            heap.puc.pace.strt_swept += k;
            let freed = heap.sweep_strings_upto(heap.puc.pace.strt_swept);
            heap.puc.pace.estimate -= freed;
            if heap.puc.pace.strt_swept >= heap.puc.strt_size as i64 {
                h.heap().end_string_sweep();
                h.heap().puc.pace.enter(St::SweepAll);
            }
            lim -= k * SWEEPCOST_51;
        } else {
            lim -= singlestep_51(h)?;
        }
        if h.heap().puc.pace.state() == St::Pause || lim <= 0 {
            break;
        }
    }
    let heap = h.heap();
    let bytes = heap.bytes() as i64;
    let pause = heap.puc.params.get(Param::Pause);
    let p = &mut heap.puc.pace;
    if p.state() != St::Pause {
        if p.dept < STEPSIZE_51 {
            p.set_threshold(bytes + STEPSIZE_51);
        } else {
            p.dept -= STEPSIZE_51;
            p.set_threshold(bytes);
        }
    } else {
        p.set_threshold(threshold_51(p.estimate, pause));
    }
    Ok(())
}

/// 5.1's `atomic`, from the step that finds no gray object.
pub(super) fn atomic_51<H: StepHost + ?Sized>(h: &mut H) {
    h.atomic();
    h.heap().puc.pace.enter(St::SweepString);
    // `totalbytes - udsize`: what is to be finalized is not counted
    h.heap().puc.pace.estimate = h.heap().bytes() as i64 - h.heap().puc.udsize;
    h.heap().puc.pace.strt_swept = 0;
}

pub(super) fn singlestep_51<H: StepHost + ?Sized>(h: &mut H) -> Result<i64, LuaError> {
    Ok(match h.heap().puc.pace.state() {
        St::Pause => {
            restart(h);
            0
        }
        St::Propagate => {
            atomic_51(h);
            0
        }
        St::SweepString => {
            let heap = h.heap();
            heap.puc.pace.strt_swept += 1;
            let freed = heap.sweep_strings_upto(heap.puc.pace.strt_swept);
            heap.puc.pace.estimate -= freed;
            if heap.puc.pace.strt_swept >= heap.puc.strt_size as i64 {
                h.heap().end_string_sweep();
                h.heap().puc.pace.enter(St::SweepAll);
            }
            SWEEPCOST_51
        }
        St::SweepAll => {
            // what `checkSizes` frees at the end is taken off too
            let old = h.heap().bytes() as i64;
            h.sweep_quota(SWEEPMAX_51);
            if !h.heap().gc_phase_is_sweep() {
                h.heap().acct_check_sizes();
                h.heap().puc.pace.enter(St::Finalize);
            }
            h.heap().puc.pace.estimate -= old - h.heap().bytes() as i64;
            SWEEPMAX_51 * SWEEPCOST_51
        }
        _ => {
            if h.heap().has_tobefnz() {
                h.run_finalizer(true)?;
                let p = &mut h.heap().puc.pace;
                if p.estimate > FINALIZECOST_51 {
                    p.estimate -= FINALIZECOST_51;
                }
                return Ok(FINALIZECOST_51);
            }
            h.heap().puc.pace.enter(St::Pause);
            h.heap().puc.pace.dept = 0;
            0
        }
    })
}
