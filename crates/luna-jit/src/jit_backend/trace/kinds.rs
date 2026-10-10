//! What the lowerer knows about each register's value as it walks a trace.

use super::*;

/// Per-register *current* kind tracked during the lowerer's forward
/// sweep. Initial values come from the trace's `entry_tags` snapshot for
/// the registers the trace reads before writing, then arith / Move / GetX /
/// NewTable writers refine them. Used by arith / cmp emit to pick the right
/// IR (iadd vs fadd; icmp vs fcmp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegKind {
    /// Never written and nothing known: a slot past the head frame
    /// (inlined frames, scratch), which reg_state starts as zero, or an
    /// entry slot whose tag no kind describes.
    Unset,
    /// Written by the trace with a value of unknown type (a table read
    /// whose result could not be inferred, a generic-for call the helper
    /// stored to the stack). Code that has to tag the value gives up or
    /// takes the trace off dispatch.
    Unknown,
    /// A head-frame register the trace does not read before writing it:
    /// the dispatcher does not check it on entry and vm.stack holds its
    /// value until the trace writes the register. Exits leave the stack
    /// slot as it is and spills skip it.
    StackHeld,
    Int,
    Float,
    Table,
    /// Lua closure pointer (raw payload is a
    /// `Gc<LuaClosure>` ptr). Produced today only by `Op::GetUpval`
    /// when `infer_upval_exit` pins the use site as Op::Call's
    /// target.
    Closure,
    Nil,
    /// interned string pointer (raw payload is a
    /// `Gc<LuaStr>` ptr). Produced by an entry slot tagged STR
    /// (via `from_entry_tag`), `LoadK` of a Str constant, or
    /// `Op::Move` propagation from a Str slot. Lets `Op::Concat`
    /// emit's operand spill pick `raw::STR` instead of going
    /// through the tag-preserving `update_raw` helper which can't
    /// handle a stale vm.stack tag.
    Str,
    /// a boolean: payload 0 (false) or 1 (true); its value tag is
    /// `raw::FALSE` plus the payload, known only at run time
    Bool,
}

impl RegKind {
    pub(super) fn from_entry_tag(tag: u8) -> Option<Self> {
        match tag {
            luna_core::runtime::value::raw::INT => Some(RegKind::Int),
            luna_core::runtime::value::raw::FLOAT => Some(RegKind::Float),
            luna_core::runtime::value::raw::TABLE => Some(RegKind::Table),
            luna_core::runtime::value::raw::CLOSURE => Some(RegKind::Closure),
            luna_core::runtime::value::raw::NIL => Some(RegKind::Nil),
            luna_core::runtime::value::raw::STR => Some(RegKind::Str),
            luna_core::runtime::value::raw::FALSE | luna_core::runtime::value::raw::TRUE => {
                Some(RegKind::Bool)
            }
            _ => None,
        }
    }

    /// The value's type is not known here, so it cannot be tagged.
    pub(super) fn untyped(self) -> bool {
        matches!(self, RegKind::Unset | RegKind::Unknown | RegKind::StackHeld)
    }
}

pub(super) fn kinds_to_exit_tags(kinds: &[RegKind]) -> Vec<ExitTag> {
    kinds.iter().map(exit_tag_of).collect()
}

/// The exit tags of `kinds`, sharing the slice of the exit before when
/// that exit left with the same kinds (`last`: its kinds and tags).
pub(super) fn shared_exit_tags<'k>(
    kinds: &'k [RegKind],
    last: &mut Option<(&'k [RegKind], TArc<[ExitTag]>)>,
) -> TArc<[ExitTag]> {
    if let Some((k, t)) = last
        && *k == kinds
    {
        return t.clone();
    }
    let tags: TArc<[ExitTag]> = kinds.iter().map(exit_tag_of).collect();
    *last = Some((kinds, tags.clone()));
    tags
}

fn exit_tag_of(k: &RegKind) -> ExitTag {
    match k {
        RegKind::Unset | RegKind::Unknown | RegKind::StackHeld => ExitTag::Untouched,
        RegKind::Int => ExitTag::Int,
        RegKind::Float => ExitTag::Float,
        RegKind::Table => ExitTag::Table,
        RegKind::Closure => ExitTag::Closure,
        RegKind::Nil => ExitTag::Nil,
        RegKind::Str => ExitTag::Str,
        RegKind::Bool => ExitTag::Bool,
    }
}

/// Whether a loop's back-edge may run the body again with the registers
/// of kinds `tail`: the body was lowered for `head`. A register held on
/// the stack at the head is one the looping body never writes (see
/// `entry_live`), so it takes whatever kind the tail reports.
pub(super) fn loop_kinds_match(tail: &[RegKind], head: &[RegKind]) -> bool {
    tail.len() == head.len()
        && tail
            .iter()
            .zip(head)
            .all(|(t, h)| *h == RegKind::StackHeld || t == h)
}

/// map a [`RegKind`] to its matching
/// [`luna_core::runtime::value::raw`] tag byte for the materialise
/// helper's per-slot kind array. Untyped slots become `NIL` so the
/// helper writes Nil into the corresponding array index — matches
/// Lua's "table created with array part, slot unwritten" semantics.
pub(super) fn kind_to_raw_tag(k: RegKind) -> u8 {
    use luna_core::runtime::value::raw;
    match k {
        RegKind::Int => raw::INT,
        RegKind::Float => raw::FLOAT,
        RegKind::Table => raw::TABLE,
        RegKind::Closure => raw::CLOSURE,
        RegKind::Str => raw::STR,
        RegKind::Unset | RegKind::Unknown | RegKind::StackHeld | RegKind::Nil => raw::NIL,
        RegKind::Bool => unreachable!("a boolean's tag is computed (`emit_kind_tag`)"),
    }
}

/// The value tag (`runtime::value::raw`) of a register of kind `k`, or
/// `None` when the kind does not say.
pub(super) fn known_tag(k: RegKind) -> Option<u8> {
    use luna_core::runtime::value::raw;
    match k {
        RegKind::Int => Some(raw::INT),
        RegKind::Float => Some(raw::FLOAT),
        RegKind::Table => Some(raw::TABLE),
        RegKind::Closure => Some(raw::CLOSURE),
        RegKind::Str => Some(raw::STR),
        RegKind::Nil => Some(raw::NIL),
        RegKind::Unset | RegKind::Unknown | RegKind::StackHeld | RegKind::Bool => None,
    }
}

/// The value tag of a register of kind `k` holding `payload`, as an `i64`
/// value: a constant, or for a boolean `raw::FALSE` plus its payload.
/// `None` when the kind does not say.
pub(super) fn emit_kind_tag<E: Ins>(bcx: &mut E, k: RegKind, payload: Value) -> Option<Value> {
    if k == RegKind::Bool {
        let f = i64::from(luna_core::runtime::value::raw::FALSE);
        return Some(bcx.ins().iadd_imm_u(payload, f));
    }
    known_tag(k).map(|t| bcx.ins().iconst(types::I64, i64::from(t)))
}

/// The value tag (`runtime::value::raw`) of a register of kind `k`.
pub(super) fn kind_tag(k: RegKind) -> u8 {
    known_tag(k).expect("callers do not lower a store of an untyped kind")
}

/// Find a register's kind by walking back from `idx-1` through the
/// already-emitted writers in `current_kinds`. Avoids re-deriving
/// the kind from scratch on every operand access.
pub(super) fn k_op(current_kinds: &[RegKind], reg: u32) -> RegKind {
    *current_kinds.get(reg as usize).unwrap_or(&RegKind::Unset)
}

/// The kinds each tagged exit leaves with, in one buffer: `(pc, at, len)`
/// per exit, its kinds at `kinds[at..at + len]`, and the cell its side
/// trace goes in (the code holds the cell's address, so it never moves).
#[derive(Default)]
pub(super) struct ExitKinds {
    kinds: Vec<RegKind>,
    exits: Vec<(u32, u32, u32)>,
    // boxed: compiled code holds each cell's address, which a growing
    // `Vec<TCellPtr>` would move
    #[allow(clippy::vec_box)]
    cells: Vec<Box<TCellPtr>>,
}

impl ExitKinds {
    /// Room for `exits` exits of `width` registers before growing.
    pub(super) fn with_capacity(exits: usize, width: usize) -> ExitKinds {
        ExitKinds {
            kinds: Vec::with_capacity(exits * width),
            exits: Vec::with_capacity(exits),
            cells: Vec::with_capacity(exits),
        }
    }

    /// Records an exit to `pc` leaving with `kinds`; its index.
    pub(super) fn push(&mut self, pc: u32, kinds: &[RegKind], cell: Box<TCellPtr>) -> u32 {
        let at = self.kinds.len() as u32;
        self.kinds.extend_from_slice(kinds);
        self.exits.push((pc, at, kinds.len() as u32));
        self.cells.push(cell);
        (self.exits.len() - 1) as u32
    }

    /// The kinds exit `k` leaves with.
    pub(super) fn kinds_mut(&mut self, k: u32) -> &mut [RegKind] {
        let (_, at, len) = self.exits[k as usize];
        &mut self.kinds[at as usize..(at + len) as usize]
    }

    /// Each exit's pc and kinds, in order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (u32, &[RegKind])> {
        self.exits
            .iter()
            .map(|&(pc, at, len)| (pc, &self.kinds[at as usize..(at + len) as usize]))
    }

    /// The side-trace cells, in exit order.
    #[allow(clippy::vec_box)]
    pub(super) fn into_cells(self) -> Vec<Box<TCellPtr>> {
        self.cells
    }
}
