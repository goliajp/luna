//! PUC `luaC_fullgc`: the collection the program asks for, and the one an
//! allocation that failed runs, which runs no finalizer and keeps what was
//! made since the last safe point (`young`).

use super::*;
use crate::runtime::puc::for_heap::ShrinkAt;

/// The objects made since the last safe point (see `Heap::young_len`).
pub(crate) type Young = (Option<usize>, bool);

/// A full collection, finishing an incremental cycle in progress first;
/// `emergency` with the young objects for one an allocation runs, which
/// only finishes a cycle in progress when there is one. Returns the number
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
    let propagating = h.heap().gc_phase_is_propagate();
    if propagating {
        // 5.3 shrinks the running thread as it traverses it, if this cycle
        // has not yet
        if program && rules.shrink_at == ShrinkAt::Traverse && !h.running_shrunk() {
            h.shrink_stacks();
        }
        let mut m = h.heap().loan_marker();
        m.enter_atomic();
        h.mark_roots(&mut m, true);
        if let Some(young) = emergency {
            h.heap().mark_young(&mut m, young);
        }
        h.heap().remark_open_upvals(&mut m);
        h.heap().stash_marker(m);
        h.marking_ends();
        h.heap().gc_finish_atomic();
        if program && rules.shrink_at != ShrinkAt::Sweep {
            h.shrink_threads();
        }
        h.collection_ends();
    }
    h.heap().gc_finish_sweep();
    // an emergency finishes the cycle in progress and no more
    let freed = if program {
        if swept {
            h.heap().acct_check_sizes();
        }
        if rules.shrink_at == ShrinkAt::Sweep {
            full_cycle_52(h, swept)
        } else {
            full_mark_and_sweep(h, None)
        }
    } else if !propagating {
        full_mark_and_sweep(h, emergency)
    } else {
        0
    };
    if program || !propagating {
        h.collection_ends();
    }
    if !program && rules.shrink_at == ShrinkAt::Sweep {
        h.shrink_threads_emergency();
    }
    freed
}

/// The stop-the-world marking and sweep of every dialect but 5.2, whose
/// full cycle runs its steps; an emergency's too.
fn full_mark_and_sweep<H: StepHost + ?Sized>(h: &mut H, emergency: Option<Young>) -> usize {
    let rules = *h.heap().puc_rules();
    gc_count_marked(h);
    let mut m = h.heap().gc_begin_full();
    h.mark_roots(&mut m, false);
    if let Some(young) = emergency {
        h.heap().mark_young(&mut m, young);
    }
    h.marking_ends();
    h.heap().mark_from(&mut m);
    if emergency.is_some() {
        return h.heap().gc_sweep_full();
    }
    // the running threads shrink as the cycle traverses them, after the
    // sweep it finished (`checkSizes`), the rest after its own sweep
    h.shrink_stacks();
    let freed = h.heap().gc_sweep_full();
    if rules.shrink_at != ShrinkAt::Sweep {
        h.shrink_threads();
    }
    h.heap().acct_check_sizes();
    freed
}
