//! The metamethod events and their names.

use super::*;
use crate::runtime::puc::api::PucParse;

/// Outcome of an index/newindex/comparison fast path: either a directly
/// computed result, or a metamethod (with the receiver it resolved against) the
/// caller must invoke — synchronously (C context) or yieldably (VM opcode).
pub(crate) enum MmOut {
    /// index → the looked-up value; newindex → done (raw set performed);
    /// comparison → the boolean result already known
    Done(Value),
    /// a metamethod to call; `recv` is the chain element it was found on (the
    /// extra args — key / value — are supplied by the caller)
    Mm { func: Value, recv: Value },
    /// ≤5.3 `a <= b` synthesised via `not __lt(b, a)` when neither operand
    /// carries `__le` — `op_compare` swaps the args and negates the result.
    /// Lives separate from `Mm` so the synth path can stay yieldable without
    /// every other Mm caller learning a swap flag they would never set.
    CompareSynth { func: Value },
}

/// Metamethod events; discriminants index `Vm::mm_names`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Mm {
    Index,
    NewIndex,
    Call,
    ToString,
    Metatable,
    Name,
    Eq,
    Lt,
    Le,
    Concat,
    Len,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Unm,
    BNot,
    Close,
    Gc,
    Pairs,
}

// one absent bit per event in `Table::flags`, below the read-only bit
const _: () = assert!(MM_NAMES.len() <= 31);

pub(crate) const MM_NAMES: [&str; 28] = [
    "__index",
    "__newindex",
    "__call",
    "__tostring",
    "__metatable",
    "__name",
    "__eq",
    "__lt",
    "__le",
    "__concat",
    "__len",
    "__add",
    "__sub",
    "__mul",
    "__div",
    "__mod",
    "__pow",
    "__idiv",
    "__band",
    "__bor",
    "__bxor",
    "__shl",
    "__shr",
    "__unm",
    "__bnot",
    "__close",
    "__gc",
    "__pairs",
];

/// The metamethod event an opcode dispatches, without the `__` prefix (PUC
/// funcnamefromcode), for "(metamethod 'event')" call-error suffixes.
pub(crate) fn mm_event_name(op: crate::vm::isa::Op) -> Option<&'static str> {
    use crate::vm::isa::Op;
    Some(match op {
        Op::Add => "add",
        Op::Sub => "sub",
        Op::Mul => "mul",
        Op::Div => "div",
        Op::Mod => "mod",
        Op::Pow => "pow",
        Op::IDiv => "idiv",
        Op::BAnd => "band",
        Op::BOr => "bor",
        Op::BXor => "bxor",
        Op::Shl => "shl",
        Op::Shr => "shr",
        Op::Unm => "unm",
        Op::BNot => "bnot",
        Op::Concat => "concat",
        Op::Len => "len",
        Op::GetField | Op::GetTable | Op::GetTableK | Op::GetTabUpR | Op::GetI | Op::SelfOp => {
            "index"
        }
        Op::SetField | Op::SetTable | Op::SetTableK | Op::SetTabUpR | Op::SetTabUpK | Op::SetI => {
            "newindex"
        }
        Op::Eq | Op::EqK => "eq",
        Op::Lt => "lt",
        Op::Le => "le",
        _ => return None,
    })
}

/// The names of the events, made for a new state of `version`: the ones
/// PUC fixes, and the others as PUC makes them, where the program first
/// uses each (`Heap::try_intern_name`).
pub(crate) fn make_names(
    heap: &mut Heap,
    version: LuaVersion,
) -> Option<[Gc<crate::runtime::LuaStr>; MM_NAMES.len()]> {
    let fixed = super::super::gc::fixed_names(version);
    let mut names = [None; MM_NAMES.len()];
    for (i, (slot, name)) in names.iter_mut().zip(MM_NAMES).enumerate() {
        *slot = Some(
            heap.try_intern_name(name.as_bytes(), fixed >> i & 1 == 0)
                .ok()?,
        );
    }
    Some(names.map(|n| n.expect("every name was made above")))
}
