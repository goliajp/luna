//! What a filled side cell has the code do, without addresses: the checks
//! before its side trace runs, the stores when it goes on in the parent's
//! loop, and the same for the trace it links to. The optimizing tier
//! compiles these into the parent's code instead of handing the cell's
//! lists to the Vm (see `crate::vm::exec::trace::relink`).
//!
//! A plan follows from the parent's and the side trace's tags alone, so two
//! Vms that fill the same cell make equal plans, and code compiled for one
//! is code for the other.

use crate::runtime::value::{raw, tag};

/// See the module comment. Slots are the frame's registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellPlan {
    /// Each slot to check on the stack, with the raw tag it must hold.
    pub checks: Box<[(u32, u8)]>,
    /// Each slot to store to the stack from its register, with the exit
    /// tag of what the register holds.
    pub stores: Box<[(u32, u8)]>,
    /// The side trace's registers may go on in the optimizing tier's loop.
    pub loop_ok: bool,
    /// The trace the side trace links to, when it does.
    pub link: Option<LinkPlan>,
}

/// The link of a [`CellPlan`]: the checks and the stores before the linked
/// trace is entered, as a [`CellPlan`]'s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPlan {
    /// The linked trace's head.
    pub pc: u32,
    /// As [`CellPlan::checks`].
    pub checks: Box<[(u32, u8)]>,
    /// As [`CellPlan::stores`].
    pub stores: Box<[(u32, u8)]>,
    /// The link as the trail notes it: the typed exit, the running trace
    /// and the linked trace (the traces by their index in the frame's
    /// prototype's list).
    pub trail: [u32; 3],
    /// The running trace's window, the linked trace's, and the frame's
    /// registers.
    pub windows: [u32; 3],
    /// The running trace counts its move to the optimizing tier in this
    /// call's word, as does the linked trace.
    pub counts: [bool; 2],
    /// The linked trace has side cells.
    pub target_cells: bool,
    /// The linked trace has a move to the optimizing tier.
    pub target_tiers: bool,
    /// The depth of the calls the linked trace inlines.
    pub target_inline_depth: u8,
}

/// The pairs of a cell's list (`relink::list::list_of`'s layout).
// out of line, for the same reason as `relink::onward::fast::fill`
#[inline(never)]
pub(crate) fn pairs_of(list: &[u32]) -> Box<[(u32, u8)]> {
    (list.get(3..).unwrap_or_default().iter())
        .map(|&e| (e >> 8, e as u8))
        .collect()
}

/// A stack value's size and where its payload sits, in bytes; its tag byte
/// is the first.
pub const VALUE_SIZE: i32 = size_of::<crate::runtime::Value>() as i32;
/// See [`VALUE_SIZE`].
pub const VALUE_PAYLOAD: i32 = 8;

/// How the code writes the value an exit tag gives a register's payload
/// into a stack slot, as the exit's write-back does (`Value::pack_into`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotStore {
    /// Nothing: the exit leaves the slot.
    Leave,
    /// The tag byte only (nil).
    Tag(u8),
    /// The tag byte, and the payload's lowest bit as the byte at
    /// [`VALUE_PAYLOAD`] (a boolean).
    Bool(u8),
    /// The tag byte, and the payload as the word at [`VALUE_PAYLOAD`].
    Word(u8),
}

/// [`SlotStore`] of exit tag `d` (a discriminant of `ExitTag`).
pub fn slot_store(d: u8) -> SlotStore {
    match crate::vm::exec::exit_raw(d) {
        None => SlotStore::Leave,
        Some(None) => SlotStore::Bool(tag::BOOL),
        Some(Some(raw::NIL)) => SlotStore::Tag(tag::NIL),
        Some(Some(r)) => SlotStore::Word(r - 1),
    }
}

/// How the code checks a stack slot holds raw tag `r`: its tag byte, and
/// for a boolean the byte at [`VALUE_PAYLOAD`] too.
pub fn slot_check(r: u8) -> (u8, Option<u8>) {
    match r {
        raw::NIL => (tag::NIL, None),
        raw::FALSE => (tag::BOOL, Some(0)),
        raw::TRUE => (tag::BOOL, Some(1)),
        r => (r - 1, None),
    }
}

/// What the exit's write-back leaves in a slot for exit tag `d` and payload
/// `r`, read back from the value it makes: the tag byte, and the payload
/// as [`SlotStore`] says it is written (a byte for a boolean, a word, or
/// none for nil). `None` when the exit leaves the slot. For tests of the
/// code [`slot_store`] has made.
pub fn slot_store_reference(d: u8, r: i64) -> Option<(u8, Option<u64>)> {
    Some(slot_bytes(crate::vm::exec::exit_value_reference(d, r)?))
}

/// What a slot holding `v`, one an exit tag gives, reads as: its tag byte,
/// and its payload as [`SlotStore`] says it is written.
pub(crate) fn slot_bytes(v: crate::runtime::Value) -> (u8, Option<u64>) {
    use crate::runtime::Value as V;
    let payload = match v {
        V::Nil => None,
        V::Bool(b) => Some(u64::from(b)),
        V::Int(i) => Some(i as u64),
        V::Float(f) => Some(f.to_bits()),
        V::Str(s) => Some(s.as_ptr() as u64),
        V::Table(t) => Some(t.as_ptr() as u64),
        V::Closure(c) => Some(c.as_ptr() as u64),
        V::Native(n) => Some(n.as_ptr() as u64),
        _ => unreachable!("no exit tag gives {}", v.type_name()),
    };
    (v.tag_byte(), payload)
}
