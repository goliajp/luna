//! What the baseline tier's call stub reads to take a link itself: the
//! words a side cell's [`SideCell::link_fast`] points at, made once when the
//! cell is filled with a link the stub may take (see `relink::fast`).
//!
//! The words are 32 bits each: a head at the offsets [`LinkFastOffsets`]
//! gives, which starts as a list for `luna_jit_side_cell_list` (of the kind
//! that checks, in a debug build, what the stub did), then `checks` pairs `(byte in the frame, byte it must hold)`, then
//! `stores` triples `(byte in the frame, byte in reg_state, tag | kind << 8)`
//! with the kinds [`STORE_TAG`], [`STORE_BOOL`] and [`STORE_WORD`].
//!
//! [`SideCell::link_fast`]: super::side_cell::SideCell

/// A store of the tag byte only.
pub const STORE_TAG: u32 = 0;
/// A store of the tag byte and the register's lowest bit as the payload.
pub const STORE_BOOL: u32 = 1;
/// A store of the tag byte and the register as the payload.
pub const STORE_WORD: u32 = 2;

/// The check pairs of `checks` (`(slot, entry tag)`, as a cell plan's):
/// one for a slot's tag byte, and one more for a boolean's payload byte.
pub fn check_items(checks: &[(u32, u8)]) -> Vec<u32> {
    use super::cell_plan::{VALUE_PAYLOAD, VALUE_SIZE, slot_check};
    let mut out = Vec::new();
    for &(slot, raw) in checks {
        let at = slot * VALUE_SIZE as u32;
        let (tag, truth) = slot_check(raw);
        out.extend([at, u32::from(tag)]);
        if let Some(t) = truth {
            out.extend([at + VALUE_PAYLOAD as u32, u32::from(t)]);
        }
    }
    out
}

/// Where the number of pairs is in the words a cell's `check_fast` points
/// at, in bytes: past a list for `luna_jit_side_cell_list` (of the kind
/// that panics, for a debug build's stub to report a disagreement).
pub const CHECK_COUNT: i32 = 12;
/// Where the pairs start in those words, in bytes.
pub const CHECK_ITEMS: i32 = 16;

/// The store triples of `stores` (`(slot, exit tag)`, as a cell plan's),
/// the slots the exit leaves left out.
// inlined into its callers, for the same reason as
// `relink::onward::fast::fill`
#[inline(always)]
pub fn store_items(stores: &[(u32, u8)]) -> Vec<u32> {
    use super::cell_plan::{SlotStore, VALUE_SIZE, slot_store};
    let mut out = Vec::new();
    for &(slot, d) in stores {
        let (tag, kind) = match slot_store(d) {
            SlotStore::Leave => continue,
            SlotStore::Tag(t) => (t, STORE_TAG),
            SlotStore::Bool(t) => (t, STORE_BOOL),
            SlotStore::Word(t) => (t, STORE_WORD),
        };
        out.extend([
            slot * VALUE_SIZE as u32,
            8 * slot,
            u32::from(tag) | kind << 8,
        ]);
    }
    out
}

/// Where the stub finds each word of the head, in bytes.
#[derive(Clone, Copy, Debug)]
pub struct LinkFastOffsets {
    /// The link as the trail notes it: three words, the last three of the
    /// list.
    pub trail: i32,
    /// The linked trace's move's `reused_at`, `at` and `calls_at`
    /// (`u32::MAX`, `u32::MAX`, 0 without a move).
    pub reused_at: i32,
    /// See `reused_at`.
    pub at: i32,
    /// See `reused_at`.
    pub calls_at: i32,
    /// The running trace's count word in `reg_state`, in bytes.
    pub count_out: i32,
    /// The linked trace's count word in `reg_state`, in bytes; its pass
    /// mark is the word after it.
    pub count_in: i32,
    /// The linked trace's side-trace note in `reg_state`, in bytes.
    pub ran_side: i32,
    /// 1 when the linked trace has side cells (its note is cleared).
    pub has_cells: i32,
    /// The number of check pairs.
    pub checks: i32,
    /// The number of store triples.
    pub stores: i32,
    /// Where the pairs start.
    pub items: i32,
}

/// See [`LinkFastOffsets`].
pub const fn offsets() -> LinkFastOffsets {
    LinkFastOffsets {
        trail: 12,
        reused_at: 24,
        at: 28,
        calls_at: 32,
        count_out: 36,
        count_in: 40,
        ran_side: 44,
        has_cells: 48,
        checks: 52,
        stores: 56,
        items: 60,
    }
}
