//! PUC `luaC_fullgc`: the collection the program asks for, and the one an
//! allocation that failed runs, which runs no finalizer and keeps what was
//! made since the last safe point (`young`).

use super::*;
use crate::runtime::puc::for_heap::ShrinkAt;

/// The objects made since the last safe point (see `Heap::young_len`).
pub(in crate::runtime::puc) type Young = (Option<usize>, bool);

/// The stop-the-world marking and sweep of every dialect but 5.2, whose
/// full cycle runs its steps; an emergency's too.
pub(in crate::runtime::puc) fn full_mark_and_sweep<H: StepHost + ?Sized>(
    h: &mut H,
    emergency: Option<Young>,
) -> usize {
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
