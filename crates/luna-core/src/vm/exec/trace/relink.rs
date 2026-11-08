//! A side trace its parent's code runs from a side cell (see
//! `crate::jit::trace_types::side_cell`): when the cell may hold it, what
//! it holds, and the stores to the stack when the side trace goes on in
//! the parent's loop.
//!
//! Going on in the parent's loop is entering the parent again without the
//! dispatcher: the parent's code goes back to where its entry block ends,
//! before its checks ahead of the loop. So the cell holds a side trace
//! only when the dispatcher would let the parent be entered with what the
//! side trace leaves: the registers the parent checks on entry of the tags
//! it checks for. And since the dispatcher does not restore the registers
//! in between, the slots that the parent's exits take as still holding
//! what the parent was entered with get stored to the stack.

use super::*;
use crate::jit::TCellU32;
use crate::jit::trace::{CompiledTrace, ENTRY_TAG_ANY, ExitTag};
use crate::jit::trace_types::exit_tag_raw;
use crate::jit::trace_types::side_cell::SideCell;

mod enter;
mod list;
mod onward;

pub(in crate::vm::exec) use onward::link_late;

use list::{SideList, list_of};

/// The typed exit of `parent`'s exit `exit` (as `exit_hit_counts` numbers
/// them).
fn typed_of(parent: &CompiledTrace, exit: usize) -> Option<u32> {
    Some(exit.checked_sub(parent.per_exit_inline.len())? as u32)
}

/// The cell of `parent`'s exit `exit`, once the parent made its cells.
fn cell_of(parent: &CompiledTrace, exit: usize) -> Option<&SideCell> {
    parent.side_cell(typed_of(parent, exit)?)
}

/// The tag each slot of the head frame holds when `child`, run from
/// `parent`'s exit `exit`, ends at the parent's head: the child's, else the
/// exit's; `None` when the slot holds what the parent was entered with.
fn tags_at_relink(
    parent: &CompiledTrace,
    exit: usize,
    child: &CompiledTrace,
) -> Vec<Option<ExitTag>> {
    let n = parent.plans.entry_tags.len();
    let (at_exit, at_tail) = (parent.exit_tags_of(exit), &child.plans.exit_tags);
    let pick = |i: usize| {
        let t = at_tail.get(i).copied().unwrap_or(ExitTag::Untouched);
        let t = match t {
            ExitTag::Untouched => at_exit.get(i).copied().unwrap_or(ExitTag::Untouched),
            t => t,
        };
        (t != ExitTag::Untouched).then_some(t)
    };
    (0..n).map(pick).collect()
}

/// Whether `parent` may be entered with registers of `tags` (see
/// [`tags_at_relink`]): each it checks on entry holds the tag it checks
/// for, and each its exits take, past a back edge, to hold the kind the
/// back edge left it (`PassCount::PerCall`) holds that kind. Going on in
/// the loop counts as a pass round it, so an exit after it restores those
/// registers so.
fn enterable(parent: &CompiledTrace, tags: &[Option<ExitTag>]) -> bool {
    let checked = parent
        .plans
        .entry_tags
        .iter()
        .zip(tags)
        .all(|(&e, &t)| e == ENTRY_TAG_ANY || t.is_none_or(|t| exit_tag_raw(t) == Some(e)));
    checked
        && (parent.plans.back_edge_kinds().iter())
            .all(|&(r, k)| tags.get(r as usize).copied().flatten() == Some(k))
}

/// Whether registers of `tags` (see [`tags_at_relink`]) may go on in the
/// optimizing tier's loop of `parent`, which reads them as the back edge
/// leaves them: each of the tag the back edge gives it, or holding what the
/// parent was entered with, checked for that tag.
fn goes_on_in_loop(parent: &CompiledTrace, tags: &[Option<ExitTag>]) -> bool {
    let Some(at_loop) = &parent.loop_tags else {
        return false;
    };
    let entry = &parent.plans.entry_tags;
    at_loop
        .iter()
        .enumerate()
        .all(|(i, &l)| match (l, tags.get(i).copied().flatten()) {
            (ExitTag::Untouched, _) => true,
            (l, Some(t)) => t == l,
            (l, None) => entry.get(i).is_some_and(|&e| exit_tag_raw(l) == Some(e)),
        })
}

/// The slots `child` reads, on entry, that the dispatcher would check on
/// the stack before it runs it (`child_reads_stack_held_ok`): those the
/// parent neither checks on entry nor writes by exit `exit`, with the tag
/// the child reads them as.
fn reads_unchecked(parent: &CompiledTrace, exit: usize, child: &CompiledTrace) -> Vec<(usize, u8)> {
    let at_exit = parent.exit_tags_of(exit);
    (parent
        .plans
        .entry_tags
        .iter()
        .zip(child.plans.entry_tags.iter()))
    .enumerate()
    .filter(|&(i, (&p, &c))| {
        p == ENTRY_TAG_ANY
            && c != ENTRY_TAG_ANY
            && at_exit.get(i).is_none_or(|&t| t == ExitTag::Untouched)
    })
    .map(|(i, (_, &c))| (i, c))
    .collect()
}

/// The slots to store to the stack when the child goes on in the parent's
/// loop, with their exit tags: those some exit of the parent leaves alone
/// (the stack holding what the parent was entered with), which hold
/// something else by then.
fn stores_at_relink(parent: &CompiledTrace, tags: &[Option<ExitTag>]) -> Vec<(usize, u8)> {
    let exits = parent.plans.exit_count();
    let untouched = |i: usize| {
        (0..exits).any(|x| {
            parent
                .exit_tags_of(x)
                .get(i)
                .is_none_or(|&t| t == ExitTag::Untouched)
        })
    };
    (tags.iter().enumerate())
        .filter_map(|(i, t)| Some((i, (*t)?)))
        .filter(|&(i, _)| untouched(i))
        .map(|(i, t)| (i, t as u8))
        .collect()
}

/// What became of a side cell when a side trace was wired to its exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::vm::exec) enum Fill {
    /// The parent's code runs the side trace.
    Filled,
    /// The exit has no cell: the parent's code runs no side trace there
    /// (an exit inside an inlined function or to the head, a side trace's
    /// or a down-recursion trace's or AOT code's exit).
    NoCell,
    /// The side trace's window is wider than the parent's.
    Wider,
    /// The side trace ends at the head of a trace other than the parent,
    /// and no trace of the parent's frame it may enter starts there (yet).
    NotHead,
    /// The exit resumes in another frame.
    Frame,
    /// The dispatcher would not enter the parent (or the trace the side
    /// trace links to) with the registers the side trace leaves.
    NotEnterable,
    /// The parent's code runs the side trace and enters the trace it links
    /// to, another of the frame's.
    Linked,
    /// The side trace ends at no trace's head.
    NoLinkPc,
}

impl Fill {
    /// Whether the parent's code runs the side trace.
    pub(in crate::vm::exec) fn calls(self) -> bool {
        matches!(self, Fill::Filled | Fill::Linked)
    }
}

/// Fills the cell of `parent`'s exit `exit` for `child`, just wired there,
/// when the parent's code may run it (see the module comment); otherwise
/// the exit leaves to the dispatcher, which runs the child. `proto` is the
/// parent's, whose traces a child ending at another head links to.
pub(in crate::vm::exec) fn relink_fill(
    parent: &CompiledTrace,
    exit: usize,
    child: &CompiledTrace,
    proto: &crate::runtime::Proto,
) -> Fill {
    let Some(cell) = typed_of(parent, exit).and_then(|t| parent.side_cell_made(t)) else {
        return Fill::NoCell;
    };
    // a child no wider than the parent runs on the parent's buffer, every
    // call of which has room for it, and writes none of the words past
    // the parent's window
    if parent.downrec_link.is_some() {
        return Fill::NoCell;
    }
    if child.window_size > parent.window_size {
        return Fill::Wider;
    }
    if parent.exit_frame_offset(exit) != 0 {
        return Fill::Frame;
    }
    let f = match child.link_pc {
        Some(pc) if pc == parent.head_pc => fill_relink(parent, exit, child, cell),
        Some(pc) => onward::fill_link(parent, exit, child, cell, proto, pc),
        None => Fill::NoLinkPc,
    };
    // the optimizing tier's code may leave this exit to the dispatcher
    if f.calls()
        && let Some(t) = &parent.tier_up
    {
        t.retier();
    }
    f
}

/// `relink_fill` for a child that ends at the parent's head.
fn fill_relink(
    parent: &CompiledTrace,
    exit: usize,
    child: &CompiledTrace,
    cell: &SideCell,
) -> Fill {
    let tags = tags_at_relink(parent, exit, child);
    if !enterable(parent, &tags) {
        return Fill::NotEnterable;
    }
    fill_cell(
        cell,
        child,
        &stores_at_relink(parent, &tags),
        &reads_unchecked(parent, exit, child),
        goes_on_in_loop(parent, &tags),
    );
    Fill::Filled
}

/// Hands `child` to `cell` with its stores at going on in the parent's
/// loop, its checks before it runs and whether its registers may go on in
/// the optimizing tier's loop.
fn fill_cell(
    cell: &SideCell,
    child: &CompiledTrace,
    stores: &[(usize, u8)],
    checks: &[(usize, u8)],
    loop_ok: bool,
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
    count_for(cell, child);
    cell.entry.set(child.current_entry() as *const u8);
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
