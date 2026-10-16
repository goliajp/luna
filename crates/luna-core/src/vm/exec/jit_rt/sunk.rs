//! The tables a trace sinks: PUC makes each one, so each is counted and
//! recorded as garbage, unless the trace leaves with it.

use super::*;
use crate::runtime::table::new_table_sizes;

impl Vm {
    /// A trace's sunk site `site` makes its table this round (a `NewTable`
    /// sized by `b`, `c`, `k`): `true` when a collection step falls due
    /// with it, the trace then leaving with the table made for real;
    /// otherwise it is counted and recorded.
    #[doc(hidden)]
    pub fn jit_sunk_table(&mut self, site: usize, b: u32, c: u32, k: bool) -> bool {
        if self.jit.sunk_ghosts.len() <= site {
            self.jit.sunk_ghosts.resize(site + 1, None);
        }
        self.jit.sunk_ghosts[site] = None;
        let Some((a, h)) = new_table_sizes(b, c, k) else {
            return false;
        };
        let h = if h == 0 { 0 } else { h.next_power_of_two() };
        let size = self.heap.puc_table_bytes(a, h);
        if self.heap.gc_due_after(size) && !self.gc_finalizing {
            self.jit.gc_exit = Some(super::JIT_GC_PREV);
            return true;
        }
        // no memory for the record: the table is not counted, as when the
        // trace leaves with it before it is
        self.jit.sunk_ghosts[site] = self.heap.acct_ghost_table(a, h).ok();
        false
    }

    /// Sunk site `site`'s table grows from `old_h` nodes to `new_h` (a
    /// field it had no room for; the site has no array part).
    #[doc(hidden)]
    pub fn jit_sunk_grow(&mut self, site: usize, old_h: usize, new_h: usize) {
        if let Some(Some(t)) = self.jit.sunk_ghosts.get(site).copied() {
            self.heap.ghost_table_resize(t, 0, old_h, 0, new_h);
        }
    }

    /// A trace leaves with sunk site `site`'s table, which it makes for
    /// real now: when PUC's was counted this round, it is made without
    /// counting until [`Vm::jit_sunk_made`]. Returns whether it was.
    #[doc(hidden)]
    pub fn jit_sunk_take(&mut self, site: usize) -> bool {
        let counted = self.jit.sunk_ghosts.get(site).is_some_and(Option::is_some);
        if counted {
            self.heap.puc.silent += 1;
        }
        counted
    }

    /// The table [`Vm::jit_sunk_take`] was for is made (`t`; `None` when it
    /// could not be): when PUC's was counted, it stands for that one.
    #[doc(hidden)]
    pub fn jit_sunk_made(&mut self, site: usize, counted: bool, t: Option<Gc<Table>>) {
        if !counted {
            return;
        }
        self.heap.puc.silent -= 1;
        let g = self.jit.sunk_ghosts[site].take();
        if let (Some(g), Some(t)) = (g, t) {
            self.heap.ghost_table_taken(g);
            self.heap.set_counted(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
        }
    }
}
