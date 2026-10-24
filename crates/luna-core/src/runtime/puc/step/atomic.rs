//! PUC `atomic` and `entersweep`, as every dialect's step runs them.

use super::*;
use crate::runtime::puc::for_heap::ShrinkAt;

/// The atomic step and the way into the sweep: the work, and the objects
/// the sweep went over on the way in.
pub(crate) fn atomic<H: StepHost + ?Sized>(h: &mut H) -> (i64, i64) {
    let rules = *h.heap().puc_rules();
    h.clear_dead_stack();
    if rules.shrink_at == ShrinkAt::Atomic {
        h.shrink_stacks();
    }
    let mut run = h.running();
    if !rules.atomic_counts_ci {
        run.ci = 0;
        run.main_ci = 0;
    }
    let mut m = h.heap().loan_marker();
    if !rules.tobefnz_at_restart {
        h.heap().mark_tobefnz(&mut m);
    }
    let roots = if rules.roots_counted_apart {
        h.mark_state_roots(&mut m, true);
        let w = h.heap().drain_work(&mut m, Some(run));
        h.mark_threads_held(&mut m);
        w
    } else {
        h.mark_roots(&mut m, true);
        0
    };
    h.heap().stash_marker(m);
    h.marking_ends();
    let (work, swept) = h.heap().gc_finish_atomic_work(Some(run));
    if rules.shrink_at == ShrinkAt::Atomic {
        h.shrink_threads();
    }
    h.collection_ends();
    // there may be objects to be finalized
    h.heap().finalizers_due();
    (work + roots, swept)
}
