//! The frames compiled levels hand over, written straight onto the stack:
//! each level's registers at its base, and its pc and register count in
//! its function slot (level 0's, whose slot holds the closure, here).
//! Levels hand over deepest first, so a level skips the slots the deeper
//! ones wrote: each slot ends up as writing the levels from level 0 up
//! would leave it. Nothing is allocated: the stack has room for every
//! level compiled code enters (`Vm::jit_enter_levels`, `Vm::jit_self_level`).

use crate::runtime::Value;
use crate::vm::isa::Inst;

/// Where the levels handed over so far are.
#[derive(Clone, Copy, Default)]
pub(crate) struct HandoffMarks {
    /// levels handed over
    pub(crate) levels: u32,
    /// the slots they wrote: from the function slot of the shallowest
    /// (the deepest written last) to the end of the widest
    pub(crate) lo: u32,
    pub(crate) hi: u32,
    /// level 0's pc and register count
    pub(crate) pc0: u32,
    pub(crate) n0: u32,
}

/// A level's pc and register count, as its function slot keeps them.
pub(crate) fn record(pc: u32, n: u32) -> Value {
    Value::Int(i64::from(pc) | i64::from(n) << 32)
}

/// The pc and register count [`record`] kept.
pub(crate) fn read_record(v: Value) -> (u32, u32) {
    let Value::Int(w) = v else {
        unreachable!("a level's function slot keeps its pc and count")
    };
    (w as u32, (w >> 32) as u32)
}

/// Writes the registers `vals` of the level at `base`, skipping the slots
/// the deeper levels in `m` wrote, and marks them written.
pub(crate) fn write_level(
    stack: &mut [Value],
    m: &mut HandoffMarks,
    base: u32,
    vals: impl ExactSizeIterator<Item = Value>,
) -> u32 {
    let n = vals.len() as u32;
    debug_assert!((base + n) as usize <= stack.len(), "no room on the stack");
    let deeper = if m.levels > 0 { m.lo..m.hi } else { 0..0 };
    for (s, v) in (base..).zip(vals) {
        if !deeper.contains(&s) {
            stack[s as usize] = v;
        }
    }
    m.lo = base - 1;
    m.hi = m.hi.max(base + n);
    m.levels += 1;
    n
}

/// The base of the level above the one at `base`, which goes on at `pc`,
/// after the self call that made it.
pub(crate) fn next_base(code: &[Inst], base: u32, pc: u32) -> u32 {
    let call = code[pc as usize - 1];
    debug_assert!(
        matches!(call.op(), crate::vm::isa::Op::Call),
        "a level below another goes on after its call"
    );
    base + call.a() + 1
}
