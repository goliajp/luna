//! A side trace that ends at the head of another trace of its parent's
//! frame: the parent's code enters that trace (the linked trace) right
//! after the side trace returns, as the dispatcher would have, without the
//! interpreter in between.
//!
//! What the dispatcher checks at run time before it enters a trace is
//! checked here once, against the tags the side trace leaves: each slot the
//! linked trace checks on entry holds its tag, or holds what the parent was
//! entered with, checked by the parent for that tag, or is checked on the
//! stack when the link is taken. The slots the parent or the side trace
//! wrote that some exit of the linked trace takes as still holding what it
//! was entered with are stored to the stack when the link is taken.

use super::*;

mod fast;
use crate::jit::send_compat::TArc;
use crate::jit::send_compat::{TCellBool, TCellPtr, TCellU32};
use crate::jit::trace_types::cell_plan::{LinkPlan, pairs_of};
use crate::jit::trace_types::frame::TRACE_FRAME_TAIL;
use crate::jit::trace_types::side_cell::LINK_TAG_KEPT;

/// `relink_fill` for a child that links to `pc`, the head of another trace
/// of the parent's frame, when one is there.
pub(super) fn fill_link(
    parent: &CompiledTrace,
    exit: usize,
    child: &CompiledTrace,
    cell: &SideCell,
    proto: &crate::runtime::Proto,
    pc: u32,
    words: &crate::vm::jit_state::RunWords,
) -> Fill {
    let traces = proto.traces.borrow();
    let at = traces.iter().position(|t| std::ptr::eq(&**t, parent));
    debug_assert!(
        at.is_some(),
        "a parent linked from is not on its frame's prototype"
    );
    let found = traces
        .iter()
        .position(|t| t.head_pc == pc && t.dispatchable && t.downrec_link.is_none());
    let (Some(from), Some(to)) = (at, found) else {
        return Fill::NotHead;
    };
    // never an entry side trace: none is on `proto.traces`
    let target = &traces[to];
    let typed = (exit - parent.per_exit_inline.len()) as u32;
    let call = [
        SideList::Link as u32,
        3,
        (parent.window_size as usize + TRACE_FRAME_TAIL) as u32,
        typed,
        from as u32,
        to as u32,
    ];
    let f = link_fill(parent, exit, child, target, cell, call);
    if f == Fill::Linked {
        let calls = &proto.call_hot_count as *const std::cell::Cell<u32>;
        let link = link_plan(parent, target, cell, (proto.max_stack.into(), calls));
        fast::fill(cell, target, &link, words);
        super::fill_cell(
            cell,
            child,
            (&[], &super::reads_unchecked(parent, exit, child)),
            false,
            words,
            Some(link),
        );
    }
    f
}

/// The plan of the link `cell` was just filled with, from `parent` to
/// `target` in a frame of `max_stack` registers; with the addresses the
/// code taking it reads set in the cell.
fn link_plan(
    parent: &CompiledTrace,
    target: &CompiledTrace,
    cell: &SideCell,
    (max_stack, calls): (u32, *const std::cell::Cell<u32>),
) -> LinkPlan {
    let count = |t: &CompiledTrace| match &t.tier_up {
        Some(m) if t.call_count => (&*m.count as *const TCellU32).cast(),
        _ => std::ptr::null(),
    };
    let lc = &cell.link_cells;
    lc.target.set((target as *const CompiledTrace).cast());
    lc.counts[0].set(count(parent));
    lc.counts[1].set(count(target));
    lc.base_entry.set((&target.entry as *const TCellPtr).cast());
    lc.calls.set(calls.cast());
    match &target.tier_up {
        Some(m) => {
            lc.tier_count.set((&*m.count as *const TCellU32).cast());
            lc.tried.set((&m.tried as *const TCellBool).cast());
            lc.entry.set((&m.optimized as *const TCellPtr).cast());
        }
        None => lc.entry.set((&target.entry as *const TCellPtr).cast()),
    }
    let l = cell.link_lists.borrow();
    LinkPlan {
        pc: target.head_pc,
        checks: pairs_of(&l.1),
        stores: pairs_of(&l.2),
        trail: [l.0[3], l.0[4], l.0[5]],
        windows: [parent.window_size, target.window_size, max_stack],
        counts: [count(parent), count(target)].map(|c| !c.is_null()),
        target_cells: target.has_side_cells(),
        target_tiers: target.tier_up.is_some(),
        target_inline_depth: target.inline_depth,
    }
}

/// A trace of a prototype that holds side traces: only once one was wired
/// to it.
fn holds_side_traces(p: &TArc<CompiledTrace>) -> bool {
    p.has_any_side_wired.get() && p.has_side_cells()
}

/// Whether [`link_late`] may find a cell to link to `target`, cached now in
/// `proto`: a side trace links only to a trace the dispatcher enters, and
/// only a trace holding side traces has a cell to fill.
pub(in crate::vm::exec) fn may_link_late(
    target: &CompiledTrace,
    proto: &crate::runtime::Proto,
) -> bool {
    target.dispatchable
        && target.downrec_link.is_none()
        && proto.traces.borrow().iter().any(holds_side_traces)
}

/// Links, now that `target` is cached in `proto` (where [`may_link_late`]),
/// the side traces of the prototype's traces that end at its head and
/// whose cells were left empty because no trace was there when they were
/// wired: each such cell moves from [`Fill::NotHead`] in `counts` to what
/// became of it now. The traces whose code now runs such a side trace go
/// into `filled`.
pub(in crate::vm::exec) fn link_late(
    target: &CompiledTrace,
    proto: &crate::runtime::Proto,
    counts: &mut [u64; 8],
    filled: &mut Vec<TArc<CompiledTrace>>,
    words: &crate::vm::jit_state::RunWords,
) {
    let waiting: Vec<_> = (proto.traces.borrow().iter())
        .filter(|p| holds_side_traces(p))
        .flat_map(|p| {
            let held = p.side_children.borrow();
            (p.side_cell_exits().iter())
                .map(|e| p.per_exit_inline.len() + *e as usize)
                .filter_map(|exit| Some((exit, held.get(exit)?.clone()?)))
                .filter(|(_, c)| c.link_pc == Some(target.head_pc) && target.head_pc != p.head_pc)
                .map(|(exit, c)| (TArc::clone(p), exit, c))
                .collect::<Vec<_>>()
        })
        .collect();
    for (parent, exit, child) in waiting {
        // cells not made yet are all empty
        let empty = super::cell_of(&parent, exit)
            .is_none_or(|c| c.entry.get().is_null() && c.link_pc.get() == u32::MAX);
        if empty {
            let f = super::relink_fill(&parent, exit, &child, proto, words);
            if f != Fill::NotHead {
                counts[Fill::NotHead as usize] -= 1;
                counts[f as usize] += 1;
            }
            if f.calls() {
                filled.push(parent);
            }
        }
    }
}

/// Fills the link of `cell`, `parent`'s exit `exit`'s, for `child` ending
/// at the head of `target`, another trace of the parent's frame.
fn link_fill(
    parent: &CompiledTrace,
    exit: usize,
    child: &CompiledTrace,
    target: &CompiledTrace,
    cell: &SideCell,
    call: [u32; 6],
) -> Fill {
    // the linked trace runs on the parent's buffer, every call of which has
    // room for its window and the words past it
    if target.window_size > parent.window_size {
        return Fill::Wider;
    }
    let tags = tags_at_relink(parent, exit, child);
    let Some(check) = link_checks(parent, target, &tags) else {
        return Fill::NotEnterable;
    };
    let kept = (tags.iter())
        .map(|t| t.and_then(exit_tag_raw).unwrap_or(LINK_TAG_KEPT))
        .collect();
    let mut lists = cell.link_lists.borrow_mut();
    *lists = (
        Box::new(call),
        list_of(SideList::Check, &check),
        list_of(SideList::Store, &stores_at_relink(target, &tags)),
        kept,
        Box::new([]),
    );
    cell.link.set(lists.0.as_ptr().cast());
    drop(lists);
    cell.link_pc.set(target.head_pc);
    Fill::Linked
}

/// The slots to check on the stack when the link is taken, with the tags
/// `target` checks them for; `None` when a slot it checks on entry holds
/// another tag by then. See the module comment.
fn link_checks(
    parent: &CompiledTrace,
    target: &CompiledTrace,
    tags: &[Option<ExitTag>],
) -> Option<Vec<(usize, u8)>> {
    let mut check = Vec::new();
    for (i, &want) in target.plans.entry_tags.iter().enumerate() {
        if want == ENTRY_TAG_ANY {
            continue;
        }
        match tags.get(i).copied().flatten() {
            Some(t) if exit_tag_raw(t) == Some(want) => {}
            Some(_) => return None,
            None => match parent.plans.entry_tags.get(i).copied() {
                Some(ENTRY_TAG_ANY) => check.push((i, want)),
                Some(p) if p == want => {}
                _ => return None,
            },
        }
    }
    Some(check)
}
