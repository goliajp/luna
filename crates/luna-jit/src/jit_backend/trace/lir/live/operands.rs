//! The values an instruction reads and writes, and the blocks a block
//! goes on to.

use super::*;

/// Calls `f` with each vreg `inst` reads.
#[inline(always)]
pub(crate) fn for_uses(lir: &Lir, i: &Inst, mut f: impl FnMut(u32)) {
    let nv = lir.value_ty.len() as u32;
    match i.op {
        Op::Iconst(_)
        | Op::Reloc(_)
        | Op::Fconst(_)
        | Op::StackAddr(..)
        | Op::StackLoad(..)
        | Op::TierCount { .. } => {}
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
        Op::Jump | Op::Call | Op::Brif(_) | Op::CallIndirect => {
            if matches!(i.op, Op::Brif(_) | Op::CallIndirect) {
                f(i.a);
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
        Op::Jump => lir.block_params(i.a).iter().for_each(|&p| f(p)),
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
        Op::Jump => [i.a, NONE],
        // the guarded path is visited last, so it is laid out first
        Op::Brif(_) => [i.c, i.b],
        Op::TierCount { .. } => [i.b, i.c],
        _ => [NONE, NONE],
    }
}
