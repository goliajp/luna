//! The debt and estimate a full collection or a change of mode leaves.

use super::*;

/// Generational mode entered (`entergen`): the debt before the first
/// minor collection (`setminordebt`), 5.4 a share of the count, 5.5
/// of what was marked.
pub(in crate::runtime::puc) fn gc_minor_debt<H: StepHost + ?Sized>(h: &mut H) {
    let d = h.heap().puc.dialect;
    let bytes = h.heap().bytes();
    let debt = match d {
        Dialect::L54 => -((bytes as i64 / 100) * h.heap().puc.params.get(Param::MinorMul)),
        Dialect::L55 => {
            let estimate = h.heap().puc.pace.estimate;
            h.heap().puc.params.apply(Param::MinorMul, estimate)
        }
        _ => return,
    };
    h.heap().puc.pace.set_debt(d, bytes, debt);
    if d == Dialect::L55 {
        // `GCmarked` counts the bytes that become old from here
        h.heap().puc.pace.estimate = 0;
    }
    h.heap().refresh_gc_limit();
}

/// Back to incremental mode: 5.4 waits for its next cycle (`enterinc`);
/// 5.5 sweeps first, a step's size away (`minor2inc`).
pub(in crate::runtime::puc) fn gc_leave_generational<H: StepHost + ?Sized>(h: &mut H) {
    if h.heap().puc.dialect != Dialect::L55 || !h.heap().gc_phase_is_pause() {
        return;
    }
    h.heap().puc.pace.count_marked = false;
    h.heap().gc_sweep_all_live();
    h.heap().puc.pace.enter(St::SweepAll);
    let bytes = h.heap().bytes();
    let debt = h.heap().puc.params.apply(Param::StepSize, 100);
    h.heap().puc.pace.set_debt(Dialect::L55, bytes, debt);
    h.heap().refresh_gc_limit();
}
