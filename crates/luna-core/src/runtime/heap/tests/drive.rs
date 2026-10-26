//! Marking driven by hand, for tests that look at the colours between
//! steps.

use super::super::*;

impl Heap {
    /// [`Heap::gc_begin_propagate`] with `roots` / `extra` as the roots.
    pub(crate) fn gc_start_propagate(&mut self, roots: &[Value], extra: &[Gc<Upvalue>]) {
        let mut m = self.gc_begin_propagate();
        for &r in roots {
            m.value(r);
        }
        for &uv in extra {
            m.mark(uv);
        }
        self.stash_marker(m);
    }

    /// Drain up to `budget` gray objects (blacken + trace). Returns true if
    /// the gray queue is now empty (caller should follow up with
    /// `gc_finish_atomic`). PUC `propagatemark` budgeted loop.
    pub(crate) fn gc_step_propagate(&mut self, budget: usize) -> bool {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        let mut n = 0;
        while n < budget {
            let Some(h) = m.pop() else {
                break;
            };
            // SAFETY: `h` was popped off the gray stack, which only `Marker::header` and `barrier_back` push to, with headers of allocated objects; frees happen only in the sweep, never during propagate or between its steps
            unsafe { crate::runtime::heap::mark::blacken(h, &mut m) };
            n += 1;
        }
        let exhausted = m.stack.is_empty() && !m.overflow;
        self.stash_marker(m);
        exhausted
    }
}
