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
use crate::jit::send_compat::TArc;
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
        super::fill_cell(
            cell,
            child,
            &[],
            &super::reads_unchecked(parent, exit, child),
            false,
        );
    }
    f
}

/// Links, now that `target` is cached in `proto`, the side traces of the
/// prototype's traces that end at its head and whose cells were left empty
/// because no trace was there when they were wired: each such cell moves
/// from [`Fill::NotHead`] in `counts` to what became of it now. The traces
/// whose code now runs such a side trace go into `filled`.
pub(in crate::vm::exec) fn link_late(
    target: &CompiledTrace,
    proto: &crate::runtime::Proto,
    counts: &mut [u64; 8],
    filled: &mut Vec<TArc<CompiledTrace>>,
) {
    if !target.dispatchable || target.downrec_link.is_some() {
        return;
    }
    let waiting: Vec<_> = (proto.traces.borrow().iter())
        .filter(|p| p.has_side_cells())
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
            let f = super::relink_fill(&parent, exit, &child, proto);
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
