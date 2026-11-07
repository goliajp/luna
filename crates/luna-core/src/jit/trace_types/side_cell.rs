//! The cell a trace's exit reads to run the side trace wired to it from its
//! own code, and to go on in its own loop when that side trace ends at the
//! trace's head.
//!
//! Each exit that may take a side trace this way has its cell, which the
//! trace owns and whose address its code holds. The Vm writes the cell when
//! it wires a side trace to the exit and when that side trace moves to the
//! optimizing tier; the code only reads it, but for the counts. A Vm runs
//! on one thread at a time, and moving it to another thread orders those
//! writes before the code's reads there, as for the tier-up loop cell.

use super::*;

/// One exit's cell (see the module comment). The code reads the fields at
/// the offsets [`SideCell::offsets`] gives.
#[repr(C)]
pub struct SideCell {
    /// The side trace's entry; null while the exit runs none from the code.
    pub(crate) entry: TCellPtr,
    /// The slots to store to the stack when the side trace goes on in the
    /// parent's loop (a list for `luna_jit_side_cell_list`); null when
    /// none.
    pub(crate) back: TCellPtr,
    /// The stack slots whose tags the side trace reads unchecked by the
    /// parent, checked before it runs (a list as `back`'s); null when none.
    pub(crate) check: TCellPtr,
    /// The count the side trace moves to the optimizing tier by: its
    /// tier-up count, or [`SideCell::own`] for one that never moves.
    pub(crate) count: TCellPtr,
    /// The count at which the code leaves the side trace to the dispatcher,
    /// which moves it on.
    pub(crate) at: TCellU32,
    /// 1 when the side trace leaves the parent's loop registers of the
    /// kinds its loop is compiled for, so that it may go on in the
    /// optimizing tier's loop.
    pub(crate) loop_ok: TCellU32,
    /// The times the side trace went on in the parent's loop from here
    /// (wrapping; for statistics).
    pub(crate) relinks: TCellU32,
    /// The count of a side trace without a tier-up count.
    pub(crate) own: TCellU32,
    /// What [`SideCell::back`] and [`SideCell::check`] point at.
    pub(crate) lists: TRefLock<(Box<[u32]>, Box<[u32]>)>,
}

/// Where the code finds each field of a [`SideCell`], in bytes.
#[derive(Clone, Copy, Debug)]
pub struct SideCellOffsets {
    /// [`SideCell::entry`]'s.
    pub entry: i32,
    /// [`SideCell::back`]'s.
    pub back: i32,
    /// [`SideCell::check`]'s.
    pub check: i32,
    /// [`SideCell::count`]'s.
    pub count: i32,
    /// [`SideCell::at`]'s.
    pub at: i32,
    /// [`SideCell::loop_ok`]'s.
    pub loop_ok: i32,
    /// [`SideCell::relinks`]'s.
    pub relinks: i32,
}

impl SideCell {
    /// An empty cell: no side trace runs from it.
    pub fn new() -> Self {
        Self {
            entry: TCellPtr::null(),
            back: TCellPtr::null(),
            check: TCellPtr::null(),
            count: TCellPtr::null(),
            at: TCellU32::new(0),
            loop_ok: TCellU32::new(0),
            relinks: TCellU32::new(0),
            own: TCellU32::new(0),
            lists: TRefLock::new((Box::new([]), Box::new([]))),
        }
    }

    /// The fields' offsets.
    pub const fn offsets() -> SideCellOffsets {
        SideCellOffsets {
            entry: std::mem::offset_of!(SideCell, entry) as i32,
            back: std::mem::offset_of!(SideCell, back) as i32,
            check: std::mem::offset_of!(SideCell, check) as i32,
            count: std::mem::offset_of!(SideCell, count) as i32,
            at: std::mem::offset_of!(SideCell, at) as i32,
            loop_ok: std::mem::offset_of!(SideCell, loop_ok) as i32,
            relinks: std::mem::offset_of!(SideCell, relinks) as i32,
        }
    }

    /// The times the side trace went on in the parent's loop from here.
    pub(crate) fn relinks(&self) -> u32 {
        self.relinks.get()
    }
}

impl Default for SideCell {
    fn default() -> Self {
        Self::new()
    }
}

/// A trace's cells: per cell, the index of the typed exit it belongs to
/// (into `per_exit_tags`), and the cell.
pub type SideCells = Box<[(u32, Box<SideCell>)]>;
