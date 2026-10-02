//! Block layout and live intervals.
//!
//! Values and variable homes share one virtual-register space: value `n` is
//! vreg `n`, variable `k` is vreg `n_values + k`. Each instruction `k` in
//! layout order reads its operands at position `2k` and writes its result at
//! `2k + 1`; a block's parameters are written by the branches into it.
//!
//! Blocks are laid out in reverse post-order, the guarded path first and its
//! exits after it. In that order every path from a value's definition to a
//! use that does not take a back edge stays between the two, so a value's
//! interval is the hull of its occurrences, extended over every loop it is
//! live into. A variable is written in several places and carried around
//! loops: its interval covers every loop it occurs in.

use super::*;

#[derive(Default)]
pub(crate) struct Analysis {
    /// Reachable blocks in layout order.
    pub(crate) order: Vec<u32>,
    /// Instructions in layout order.
    pub(crate) code: Vec<u32>,
    /// Per block: `code` index of its first instruction (`NONE` if not laid
    /// out) and one past its last.
    pub(crate) block_at: Vec<(u32, u32)>,
    pub(crate) n_values: u32,
    /// Per vreg: interval `[start, end]`, `start == NONE` when never live.
    pub(crate) start: Vec<u32>,
    pub(crate) end: Vec<u32>,
    /// Per value: how many instructions read it.
    pub(crate) uses: Vec<u32>,
    /// Positions of calls (the operand-read position `2k`), ascending.
    pub(crate) calls: Vec<u32>,
    /// Variables read anywhere: they start out zero, as with Cranelift,
    /// where a variable read on a path that never wrote it is zero.
    pub(crate) entry_vars: Vec<u32>,
    // scratch, kept for the next trace
    seen: Vec<bool>,
    stack: Vec<(u32, u8)>,
    read: Vec<bool>,
    /// (loop head position, back edge position)
    loops: Vec<(u32, u32)>,
}

fn reset<T: Clone>(v: &mut Vec<T>, n: usize, x: T) {
    v.clear();
    v.resize(n, x);
}

/// Calls `f` with each vreg `inst` reads.
pub(crate) fn for_uses(lir: &Lir, i: &Inst, mut f: impl FnMut(u32)) {
    let nv = lir.value_ty.len() as u32;
    match i.op {
        Op::Iconst(_) | Op::Fconst(_) | Op::StackAddr(..) | Op::StackLoad(..) => {}
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

fn succs(lir: &Lir, b: u32) -> [u32; 2] {
    let last = lir.blocks[b as usize].last;
    if last == NONE {
        return [NONE, NONE];
    }
    let i = &lir.insts[last as usize];
    match i.op {
        Op::Jump => [i.a, NONE],
        // the guarded path is visited last, so it is laid out first
        Op::Brif(_) => [i.c, i.b],
        _ => [NONE, NONE],
    }
}

/// Reverse post-order of the blocks reachable from the entry block, into
/// `an.order`.
fn layout(lir: &Lir, an: &mut Analysis) {
    let n = lir.blocks.len();
    reset(&mut an.seen, n, false);
    an.order.clear();
    an.stack.clear();
    an.stack.push((0, 0));
    an.seen[0] = true;
    while let Some(top) = an.stack.last_mut() {
        let (b, k) = *top;
        if k == 2 {
            an.order.push(b);
            an.stack.pop();
            continue;
        }
        top.1 += 1;
        let s = succs(lir, b)[k as usize];
        if s != NONE && !an.seen[s as usize] {
            an.seen[s as usize] = true;
            an.stack.push((s, 0));
        }
    }
    an.order.reverse();
}

/// Fills `an` for `lir`, reusing its buffers.
pub(crate) fn analyze(lir: &Lir, an: &mut Analysis) {
    let nv = lir.value_ty.len() as u32;
    let nreg = (nv as usize) + lir.var_ty.len();
    layout(lir, an);
    an.code.clear();
    reset(&mut an.block_at, lir.blocks.len(), (NONE, NONE));
    for &b in &an.order {
        let first = an.code.len() as u32;
        let mut i = lir.blocks[b as usize].first;
        while i != NONE {
            an.code.push(i);
            i = lir.insts[i as usize].next;
        }
        an.block_at[b as usize] = (first, an.code.len() as u32);
    }
    reset(&mut an.start, nreg, NONE);
    reset(&mut an.end, nreg, 0);
    reset(&mut an.uses, nv as usize, 0);
    reset(&mut an.read, lir.var_ty.len(), false);
    an.calls.clear();
    an.loops.clear();
    let Analysis {
        order,
        code,
        block_at,
        start,
        end,
        uses,
        calls,
        entry_vars,
        read,
        loops,
        ..
    } = an;
    let mut touch = |r: u32, p: u32| {
        let r = r as usize;
        if start[r] == NONE {
            start[r] = p;
        }
        if p > end[r] {
            end[r] = p;
        }
    };
    for &b in order.iter() {
        let (lo, hi) = block_at[b as usize];
        for (off, &ii) in code[lo as usize..hi as usize].iter().enumerate() {
            let p = 2 * (lo + off as u32);
            let inst = &lir.insts[ii as usize];
            for_uses(lir, inst, |r| {
                if r < nv {
                    uses[r as usize] += 1;
                } else {
                    read[(r - nv) as usize] = true;
                }
                touch(r, p);
            });
            for_defs(lir, inst, |r| touch(r, p + 1));
            if matches!(inst.op, Op::Call | Op::CallIndirect) {
                calls.push(p);
            }
        }
        if hi > lo {
            for s in succs(lir, b) {
                if s != NONE && block_at[s as usize].0 <= lo {
                    loops.push((2 * block_at[s as usize].0, 2 * hi - 1));
                }
            }
        }
    }
    // the function's parameter arrives in the entry block
    if lir.arg0 != NONE {
        touch(lir.arg0, 0);
    }
    // a value nothing reads needs no register (its pure instruction is not
    // emitted; a call's result is dropped)
    for r in 0..nv as usize {
        if uses[r] == 0 {
            start[r] = NONE;
        }
    }

    entry_vars.clear();
    for (k, &r) in read.iter().enumerate() {
        if r {
            entry_vars.push(k as u32);
            start[nv as usize + k] = 0;
        }
    }
    // inner loops first, so an outer loop sees what they extended
    loops.sort_unstable_by_key(|&(h, e)| e - h);
    for &(h, e) in loops.iter() {
        for r in 0..nreg {
            let s = start[r];
            if s == NONE || s > e || end[r] < h {
                continue;
            }
            if r >= nv as usize {
                start[r] = s.min(h);
                end[r] = end[r].max(e);
            } else if s < h {
                end[r] = end[r].max(e);
            }
        }
    }
    an.n_values = nv;
}
