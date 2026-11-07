//! The values an instruction reads and writes, and the blocks a block
//! goes on to.

use super::*;

/// Calls `f` with each vreg `inst` reads.
#[inline(always)]
pub(crate) fn for_uses(lir: &Lir, i: &Inst, mut f: impl FnMut(u32)) {
    let nv = lir.value_ty.len() as u32;
    match i.op {
        Op::Iconst(_) | Op::Reloc(_) | Op::Fconst(_) | Op::StackAddr(..) | Op::StackLoad(..) => {}
        Op::TierCount {
            count: CountAt::Frame(_),
            ..
        } => f(i.a),
        Op::TierCount { .. } | Op::Pick => {}
        Op::Bin(_) | Op::Icmp(_) | Op::Fcmp(_) | Op::Store(_) => {
            f(i.a);
            f(i.b);
        }
        Op::BinImm(..) | Op::Un(_) | Op::IcmpImm(..) | Op::Load(_) | Op::Uload8(_) => f(i.a),
        Op::StackStore(..) => f(i.a),
        Op::Select => {
            f(i.a);
            f(i.b);
            f(i.c);
        }
        Op::Jump | Op::CallingExit(_) | Op::Call | Op::Brif(_) | Op::CallIndirect => {
            if matches!(i.op, Op::Brif(_) | Op::CallIndirect) {
                f(i.a);
            }
            // the baseline tier's exit hands `reg_state` to the call stub
            if matches!(i.op, Op::CallingExit(_)) {
                f(lir.arg0);
            }
            for &x in &lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize] {
                f(x);
            }
        }
        Op::Return => {
            if i.a != NONE {
                f(i.a)
            }
        }
        Op::VarRead => f(nv + i.a),
        Op::VarWrite => f(i.b),
    }
}

/// Calls `f` with each vreg `inst` writes.
#[inline(always)]
pub(crate) fn for_defs(lir: &Lir, i: &Inst, mut f: impl FnMut(u32)) {
    let nv = lir.value_ty.len() as u32;
    if i.dst != NONE {
        f(i.dst);
    }
    match i.op {
        Op::VarWrite => f(nv + i.a),
        Op::Jump | Op::CallingExit(_) => lir.block_params(i.a).iter().for_each(|&p| f(p)),
        Op::Brif(_) => {
            lir.block_params(i.b).iter().for_each(|&p| f(p));
            lir.block_params(i.c).iter().for_each(|&p| f(p));
        }
        _ => {}
    }
}

pub(crate) fn succs(lir: &Lir, b: u32) -> [u32; 2] {
    let last = lir.blocks[b as usize].last;
    if last == NONE {
        return [NONE, NONE];
    }
    let i = &lir.insts[last as usize];
    match i.op {
        Op::Jump | Op::CallingExit(_) => [i.a, NONE],
        Op::Pick => [i.b, NONE],
        // the guarded path is visited last, so it is laid out first
        Op::Brif(_) => [i.c, i.b],
        Op::TierCount { .. } => [i.b, i.c],
        _ => [NONE, NONE],
    }
}

/// Which function of a trace's instructions is compiled: the trace, or one
/// of the two the optimizing tier makes of a looping one (see
/// `lower::handoff`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Start {
    #[default]
    Whole,
    /// From `loop_entry`, each `TierCount` going on to its continuation.
    Loop,
    /// From the entry, each `TierCount` going to its hot block, which calls
    /// the loop.
    FirstPass,
}

/// The calling exits ([`Op::CallingExit`]) of a function made of a trace's
/// instructions that go on to the call block, by side cell: all of them, or
/// those whose cell held a side trace when the optimizing tier compiled the
/// function, the others returning to the dispatcher; or none, in the
/// baseline tier's code, whose exits go on to the shared call stub and
/// whose relink block is a second entry (see [`super::super::Relink`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Calls {
    #[default]
    All,
    Held(Box<[bool]>),
    Stub,
}

impl Calls {
    pub(crate) fn has(&self, n: u32) -> bool {
        match self {
            Calls::All => true,
            Calls::Held(h) => h.get(n as usize).copied().unwrap_or(false),
            Calls::Stub => false,
        }
    }
}

/// The blocks `b` goes on to in the function `start` names, its calling
/// exits going on as `calls` says: a `TierCount` to its hot block only in
/// the first pass, a `Pick` to its `a` in the loop's function.
pub(super) fn succs_of(lir: &Lir, b: u32, start: Start, calls: &Calls) -> [u32; 2] {
    let last = lir.blocks[b as usize].last;
    let Some(i) = lir.insts.get(last as usize) else {
        return [NONE, NONE];
    };
    match (i.op, start) {
        (Op::TierCount { .. }, Start::FirstPass) => [i.b, NONE],
        (Op::Pick, Start::Loop) => [i.a, NONE],
        (Op::CallingExit(n), _) if !calls.has(n) => [NONE, NONE],
        _ => succs(lir, b),
    }
}

/// The blocks the replay of the function `an` lays out goes on to from
/// `b`: as [`succs_of`], but a `TierCount` to its continuation only outside
/// the first pass (Cranelift and LLVM code does not count).
pub(crate) fn replay_succs(lir: &Lir, an: &super::Analysis, b: u32) -> [u32; 2] {
    let last = lir.blocks[b as usize].last;
    match lir.insts.get(last as usize) {
        Some(i) if matches!(i.op, Op::TierCount { .. }) && an.func != Start::FirstPass => {
            [i.c, NONE]
        }
        _ => succs_of(lir, b, an.func, &an.exits),
    }
}
