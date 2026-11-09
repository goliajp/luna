//! The words the baseline tier's call stub takes a link by (see
//! `trace_types::link_fast`): what `link_take` and `trace_entry_counted` do
//! for this link, with everything known when the cell is filled worked out
//! here, so that the stub only checks and writes.

use super::SideList;
use crate::jit::trace::CompiledTrace;
use crate::jit::trace_types::cell_plan::{LinkPlan, VALUE_PAYLOAD, VALUE_SIZE, slot_check};
use crate::jit::trace_types::frame::{TRACE_FRAME_TAIL, trace_count_slot, trace_ran_side_slot};
use crate::jit::trace_types::link_fast::store_items;
use crate::jit::trace_types::side_cell::SideCell;

/// Sets `cell`'s words for the stub to take `link` (to `target`) itself,
/// when it may: the linked trace inlines no calls (whose room only the Vm
/// knows) and its window is within the frame's registers (none to clear).
pub(super) fn fill(
    cell: &SideCell,
    target: &CompiledTrace,
    link: &LinkPlan,
    words: &crate::vm::jit_state::RunWords,
) {
    let [prev, window, max_stack] = link.windows.map(|w| w as usize);
    if link.target_inline_depth != 0 || window > max_stack || words.stub_links_off.get() {
        return;
    }
    let byte = |slot: usize| (8 * slot) as u32;
    let tier = (target.tier_up.as_ref())
        .map_or([u32::MAX, u32::MAX, 0], |t| [t.reused_at, t.at, t.calls_at]);
    let mut checks = Vec::new();
    for &(slot, raw) in &link.checks {
        let at = slot * VALUE_SIZE as u32;
        let (tag, truth) = slot_check(raw);
        checks.extend([at, u32::from(tag)]);
        if let Some(t) = truth {
            checks.extend([at + VALUE_PAYLOAD as u32, u32::from(t)]);
        }
    }
    let stores = store_items(&link.stores);
    let mut words = vec![SideList::Verify as u32, 3, (prev + TRACE_FRAME_TAIL) as u32];
    words.extend(link.trail);
    words.extend(tier);
    words.extend([
        byte(trace_count_slot(prev)),
        byte(trace_count_slot(window)),
        byte(trace_ran_side_slot(window)),
        u32::from(link.target_cells),
        (checks.len() / 2) as u32,
        (stores.len() / 3) as u32,
    ]);
    words.extend(checks);
    words.extend(stores);
    let mut lists = cell.link_lists.borrow_mut();
    lists.4 = words.into();
    cell.link_fast.set(lists.4.as_ptr().cast());
}
