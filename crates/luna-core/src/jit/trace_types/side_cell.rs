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

use super::cell_plan::CellPlan;
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
    /// What the baseline tier's call stub checks in place of `check`'s
    /// list: the number of pairs, then the pairs `link_fast::check_items`
    /// gives; null when `check` is.
    pub(crate) check_fast: TCellPtr,
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
    /// What [`SideCell::back`], [`SideCell::check`] and
    /// [`SideCell::check_fast`] point at.
    pub(crate) lists: TRefLock<(Box<[u32]>, Box<[u32]>, Box<[u32]>)>,
    /// The head of the trace the side trace links to, another trace of the
    /// same frame whose entry it was checked against; `u32::MAX` when none.
    pub(crate) link_pc: TCellU32,
    /// The list the code hands `luna_jit_side_cell_list` to enter the
    /// linked trace (in `link_lists`); null when none.
    pub(crate) link: TCellPtr,
    /// What entering the linked trace takes: the list `link` points at, the
    /// checks (as `check`'s), the stores (as `back`'s), and each slot's
    /// entry tag as the linked trace is entered ([`LINK_TAG_KEPT`] where it
    /// is the one the side trace's parent was entered with).
    pub(crate) link_lists: TRefLock<LinkLists>,
    /// What the lists say, for the optimizing tier to compile in; set when
    /// the cell is filled, never after.
    pub(crate) plan: PlanOnce,
    /// The run words of the cell's Vm (`crate::__private::RunWords`), for
    /// code shared by every Vm; set when the cell is filled.
    pub(crate) words: TCellPtr,
    /// What the code taking the link itself reads and writes of the traces
    /// (see [`LinkCells`]); set with the link.
    pub(crate) link_cells: LinkCells,
    /// What the baseline tier's call stub reads to take the link itself
    /// (see `link_fast`, in `link_lists`); null when it leaves the link to
    /// the Vm.
    pub(crate) link_fast: TCellPtr,
}

/// The addresses in the two traces of a link that the code taking it reads
/// and writes, which hold while the frame's prototype keeps them.
#[repr(C)]
pub(crate) struct LinkCells {
    /// The linked trace.
    pub(crate) target: TCellPtr,
    /// Its move's `tried` (null without a move).
    pub(crate) tried: TCellPtr,
    /// The cell holding its entry: its move's optimizing tier's, or its
    /// own without a move.
    pub(crate) entry: TCellPtr,
    /// The running trace's count of its move and the linked trace's, for
    /// one that counts in this call's word (else null).
    pub(crate) counts: [TCellPtr; 2],
    /// Its own entry, the baseline code's.
    pub(crate) base_entry: TCellPtr,
    /// Its move's count (null without a move).
    pub(crate) tier_count: TCellPtr,
    /// The frame's prototype's `call_hot_count`.
    pub(crate) calls: TCellPtr,
}

impl LinkCells {
    const fn new() -> Self {
        Self {
            target: TCellPtr::null(),
            tried: TCellPtr::null(),
            entry: TCellPtr::null(),
            counts: [TCellPtr::null(), TCellPtr::null()],
            base_entry: TCellPtr::null(),
            tier_count: TCellPtr::null(),
            calls: TCellPtr::null(),
        }
    }
}

/// A cell's plan once set.
#[cfg(not(feature = "send"))]
pub(crate) type PlanOnce = std::cell::OnceCell<std::sync::Arc<CellPlan>>;
/// A cell's plan once set (send build).
#[cfg(feature = "send")]
pub(crate) type PlanOnce = std::sync::OnceLock<std::sync::Arc<CellPlan>>;

/// The links compiled code takes in a row before it leaves the next to the
/// dispatcher: each takes a native frame of the linked trace's code.
pub const LINK_DEPTH: usize = 8;

/// In a side cell's entry tags of the linked trace: the slot holds what the
/// side trace's parent was entered with.
pub(crate) const LINK_TAG_KEPT: u8 = 0xFE;

/// Where the code finds each field of a [`SideCell`], in bytes.
#[derive(Clone, Copy, Debug)]
pub struct SideCellOffsets {
    /// `SideCell::entry`'s.
    pub entry: i32,
    /// `SideCell::back`'s.
    pub back: i32,
    /// `SideCell::check`'s.
    pub check: i32,
    /// `SideCell::check_fast`'s.
    pub check_fast: i32,
    /// `SideCell::count`'s.
    pub count: i32,
    /// `SideCell::at`'s.
    pub at: i32,
    /// `SideCell::loop_ok`'s.
    pub loop_ok: i32,
    /// `SideCell::relinks`'s.
    pub relinks: i32,
    /// `SideCell::link_pc`'s.
    pub link_pc: i32,
    /// `SideCell::link`'s.
    pub link: i32,
    /// `SideCell::words`'s.
    pub words: i32,
    /// The linked trace's address.
    pub link_target: i32,
    /// The address of the linked trace's move's `tried` (a byte).
    pub link_tried: i32,
    /// The address of the cell holding the linked trace's entry.
    pub link_entry: i32,
    /// The addresses of the running and the linked trace's counts (32 bits).
    pub link_counts: i32,
    /// The address of the linked trace's own entry cell.
    pub link_base_entry: i32,
    /// The address of the linked trace's move's count (32 bits).
    pub link_tier_count: i32,
    /// The address of the frame's prototype's call count (32 bits).
    pub link_calls: i32,
    /// `SideCell::link_fast`'s.
    pub link_fast: i32,
}

/// See [`SideCell::link_lists`].
/// The fifth is what `link_fast` points at.
pub(crate) type LinkLists = (Box<[u32]>, Box<[u32]>, Box<[u32]>, Box<[u8]>, Box<[u32]>);

impl SideCell {
    /// An empty cell: no side trace runs from it.
    pub fn new() -> Self {
        Self {
            entry: TCellPtr::null(),
            back: TCellPtr::null(),
            check: TCellPtr::null(),
            check_fast: TCellPtr::null(),
            count: TCellPtr::null(),
            at: TCellU32::new(0),
            loop_ok: TCellU32::new(0),
            relinks: TCellU32::new(0),
            own: TCellU32::new(0),
            lists: TRefLock::new(Default::default()),
            link_pc: TCellU32::new(u32::MAX),
            link: TCellPtr::null(),
            link_lists: TRefLock::new(Default::default()),
            plan: PlanOnce::new(),
            words: TCellPtr::null(),
            link_cells: LinkCells::new(),
            link_fast: TCellPtr::null(),
        }
    }

    /// The fields' offsets.
    pub const fn offsets() -> SideCellOffsets {
        let lc = std::mem::offset_of!(SideCell, link_cells);
        SideCellOffsets {
            entry: std::mem::offset_of!(SideCell, entry) as i32,
            back: std::mem::offset_of!(SideCell, back) as i32,
            check: std::mem::offset_of!(SideCell, check) as i32,
            check_fast: std::mem::offset_of!(SideCell, check_fast) as i32,
            count: std::mem::offset_of!(SideCell, count) as i32,
            at: std::mem::offset_of!(SideCell, at) as i32,
            loop_ok: std::mem::offset_of!(SideCell, loop_ok) as i32,
            relinks: std::mem::offset_of!(SideCell, relinks) as i32,
            link_pc: std::mem::offset_of!(SideCell, link_pc) as i32,
            link: std::mem::offset_of!(SideCell, link) as i32,
            words: std::mem::offset_of!(SideCell, words) as i32,
            link_target: (lc + std::mem::offset_of!(LinkCells, target)) as i32,
            link_tried: (lc + std::mem::offset_of!(LinkCells, tried)) as i32,
            link_entry: (lc + std::mem::offset_of!(LinkCells, entry)) as i32,
            link_counts: (lc + std::mem::offset_of!(LinkCells, counts)) as i32,
            link_base_entry: (lc + std::mem::offset_of!(LinkCells, base_entry)) as i32,
            link_tier_count: (lc + std::mem::offset_of!(LinkCells, tier_count)) as i32,
            link_calls: (lc + std::mem::offset_of!(LinkCells, calls)) as i32,
            link_fast: std::mem::offset_of!(SideCell, link_fast) as i32,
        }
    }

    /// The cell's plan, once it is filled.
    pub fn plan(&self) -> Option<&std::sync::Arc<CellPlan>> {
        self.plan.get()
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

/// A trace's cells, in one allocation that does not move while the trace
/// lives (its code holds their addresses, cell `n` at `n` times
/// `size_of::<SideCell>()` past the first's), in the order of the typed
/// exits they belong to (see `CompiledTraceParts::side_cell_exits`).
pub type SideCells = Box<[SideCell]>;

/// `n` empty cells.
pub fn side_cells(n: usize) -> SideCells {
    (0..n).map(|_| SideCell::new()).collect()
}

/// A trace's cells once made: set at most once, and never moved after.
#[cfg(not(feature = "send"))]
pub(crate) type CellsOnce = std::cell::OnceCell<SideCells>;
/// A trace's cells once made (send build).
#[cfg(feature = "send")]
pub(crate) type CellsOnce = std::sync::OnceLock<SideCells>;
