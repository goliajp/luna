//! Filling a side cell: the lists and the plan its code reads, and the
//! count it moves the side trace on by.

use super::*;
use crate::jit::TCellU32;
use crate::jit::trace_types::cell_plan::{CellPlan, LinkPlan};
use list::{SideList, list_of};

/// Hands `child` to `cell` with its stores at going on in the parent's
/// loop and its checks before it runs, whether its registers may go on in
/// the optimizing tier's loop, and the run words of the Vm it is in.
pub(super) fn fill_cell(
    cell: &SideCell,
    child: &CompiledTrace,
    (stores, checks): (&[(usize, u8)], &[(usize, u8)]),
    loop_ok: bool,
    words: &crate::vm::jit_state::RunWords,
    link: Option<LinkPlan>,
) {
    let mut lists = cell.lists.borrow_mut();
    *lists = (
        list_of(SideList::Store, stores),
        list_of(SideList::Check, checks),
    );
    let at = |l: &[u32]| {
        if l.is_empty() {
            std::ptr::null()
        } else {
            l.as_ptr().cast()
        }
    };
    cell.back.set(at(&lists.0));
    cell.check.set(at(&lists.1));
    drop(lists);
    cell.loop_ok.set(u32::from(loop_ok));
    cell.words
        .set((words as *const crate::vm::jit_state::RunWords).cast());
    let fresh = cell
        .plan
        .set(plan_of(stores, checks, loop_ok, link))
        .is_ok();
    debug_assert!(fresh, "a side cell filled twice");
    count_for(cell, child);
    cell.entry.set(child.current_entry() as *const u8);
}

/// The plan of a cell as `fill_cell` fills it, with its link's.
fn plan_of(
    stores: &[(usize, u8)],
    checks: &[(usize, u8)],
    loop_ok: bool,
    link: Option<LinkPlan>,
) -> std::sync::Arc<CellPlan> {
    let pairs = |l: &[(usize, u8)]| l.iter().map(|&(i, t)| (i as u32, t)).collect();
    std::sync::Arc::new(CellPlan {
        checks: pairs(checks),
        stores: pairs(stores),
        loop_ok,
        link,
    })
}

/// Points `cell`'s count at `child`'s move to the optimizing tier: the code
/// leaves the child to the dispatcher once the count reaches the move's,
/// and never once it has moved or cannot.
fn count_for(cell: &SideCell, child: &CompiledTrace) {
    match &child.tier_up {
        Some(t) if !t.tried.get() => {
            cell.count.set((&*t.count as *const TCellU32).cast());
            cell.at.set(t.at);
        }
        Some(t) if t.optimized.get().is_null() => {
            // moving still: the dispatcher looks again at each pass
            cell.count.set((&*t.count as *const TCellU32).cast());
            cell.at.set(0);
        }
        _ => {
            cell.count.set((&cell.own as *const TCellU32).cast());
            cell.at.set(u32::MAX);
        }
    }
}

/// After the dispatcher ran `child` from `parent`'s exit `exit` (the code
/// left it the child at its move, or the cell is empty): the cell follows
/// the child's entry and move.
pub(in crate::vm::exec) fn relink_refresh(
    parent: &CompiledTrace,
    exit: usize,
    child: &CompiledTrace,
) {
    if let Some(cell) = cell_of(parent, exit)
        && !cell.entry.get().is_null()
    {
        count_for(cell, child);
        cell.entry.set(child.current_entry() as *const u8);
    }
}
