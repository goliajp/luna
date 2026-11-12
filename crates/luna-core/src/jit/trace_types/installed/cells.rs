//! A compiled trace's side cells (see `super::super::side_cell`): made
//! when a side trace is first wired to one of its calling exits, unless the
//! code was compiled holding their address.

use super::super::cell_plan::CellPlan;
use super::super::side_cell::{SideCell, side_cells};
use super::*;

impl CompiledTrace {
    /// Whether the trace has exits that may run a side trace from its code.
    #[inline]
    pub fn has_side_cells(&self) -> bool {
        !self.side_cell_exits.is_empty()
    }

    /// The typed exit of each of its cells, in their order.
    #[inline]
    pub fn side_cell_exits(&self) -> &std::sync::Arc<[u32]> {
        &self.side_cell_exits
    }

    /// The address of its first cell, once they are made.
    pub fn side_cells_at(&self) -> Option<*const SideCell> {
        self.side_cells.get().map(|c| c.as_ptr())
    }

    /// The cell of typed exit `typed`, once the cells are made.
    #[inline(always)]
    pub(crate) fn side_cell(&self, typed: u32) -> Option<&SideCell> {
        let n = self.side_cell_exits.iter().position(|&e| e == typed)?;
        self.side_cells.get()?.get(n)
    }

    /// The cell of typed exit `typed`, the cells made first when they are
    /// not yet.
    pub(crate) fn side_cell_made(&self, typed: u32) -> Option<&SideCell> {
        let n = self.side_cell_exits.iter().position(|&e| e == typed)?;
        let cells = (self.side_cells).get_or_init(|| side_cells(self.side_cell_exits.len()));
        Some(&cells[n])
    }

    /// The times side traces its code ran went on in its loop, over its
    /// side cells (each count wrapping at `u32::MAX`).
    pub fn side_relinks(&self) -> u64 {
        let cells = self.side_cells.get().map_or(&[][..], |c| &c[..]);
        cells.iter().map(|c| u64::from(c.relinks())).sum()
    }

    /// Per side cell: whether it holds a side trace's entry, and whether
    /// that is the side trace's optimizing tier's.
    pub fn side_cell_states(&self) -> Vec<(bool, bool)> {
        let Some(cells) = self.side_cells.get() else {
            return vec![(false, false); self.side_cell_exits.len()];
        };
        let held = self.side_children.borrow();
        (self.side_cell_exits.iter().zip(cells.iter()))
            .map(|(e, c)| {
                let child = held.get(self.per_exit_inline.len() + *e as usize);
                let optimized = child.cloned().flatten().and_then(|ch| {
                    let o = ch.child.tier_up.as_ref()?.optimized.get();
                    (!o.is_null()).then_some(o)
                });
                let entry = c.entry.get();
                (!entry.is_null(), optimized.is_some_and(|o| o == entry))
            })
            .collect()
    }

    /// Per side cell: its plan once it holds a side trace's entry.
    pub fn side_cell_plans(&self) -> Vec<Option<std::sync::Arc<CellPlan>>> {
        let Some(cells) = self.side_cells.get() else {
            return vec![None; self.side_cell_exits.len()];
        };
        (cells.iter())
            .map(|c| {
                (!c.entry.get().is_null())
                    .then(|| c.plan.get().cloned())
                    .flatten()
            })
            .collect()
    }
}
