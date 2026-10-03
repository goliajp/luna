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
    /// Per value: the variable it was read from when it can live in that
    /// variable's home (nothing writes the variable while the value is
    /// live), else `NONE`. Such a value gets no interval of its own.
    pub(crate) alias: Vec<u32>,
    /// Per vreg: its reads and writes, those inside loops counted
    /// `LOOP_WEIGHT` times per loop around them (what spilling it costs).
    pub(crate) weight: Vec<u32>,
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
    depth: Vec<i32>,
    writes: Vec<Vec<u32>>,
}

/// How much more a read or write inside a loop counts than one outside.
const LOOP_WEIGHT: u32 = 8;

fn reset<T: Clone>(v: &mut Vec<T>, n: usize, x: T) {
    v.clear();
    v.resize(n, x);
}

/// Calls `f` with each vreg `inst` reads.
pub(crate) fn for_uses(lir: &Lir, i: &Inst, mut f: impl FnMut(u32)) {
    let nv = lir.value_ty.len() as u32;
    match i.op {
        Op::Iconst(_)
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
        Op::TierCount { .. } => [i.b, i.c],
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
    coalesce_var_reads(lir, an);
    weigh(lir, an);
}

/// A value read from a variable that is not written while the value is
/// live stays in the variable's home: the read is no copy, and the
/// variable, now read wherever the value is, keeps a register more often.
fn coalesce_var_reads(lir: &Lir, an: &mut Analysis) {
    let nv = an.n_values as usize;
    let Analysis {
        code,
        start,
        end,
        alias,
        writes,
        ..
    } = an;
    reset(alias, nv, NONE);
    writes.iter_mut().for_each(Vec::clear);
    writes.resize_with(lir.var_ty.len(), Vec::new);
    for (k, &ii) in code.iter().enumerate() {
        let i = &lir.insts[ii as usize];
        if matches!(i.op, Op::VarWrite) {
            writes[i.a as usize].push(2 * k as u32 + 1);
        }
    }
    for (k, &ii) in code.iter().enumerate() {
        let i = &lir.insts[ii as usize];
        if !matches!(i.op, Op::VarRead) || start[i.dst as usize] == NONE {
            continue;
        }
        let d = i.dst as usize;
        let p = 2 * k as u32 + 1;
        let w = &writes[i.a as usize];
        // the first write after the read, in layout order: loops extended
        // the value's interval over every write that can run while it lives
        let next = w.partition_point(|&x| x <= p);
        if w.get(next).is_some_and(|&x| x <= end[d]) {
            continue;
        }
        let r = nv + i.a as usize;
        end[r] = end[r].max(end[d]);
        start[r] = start[r].min(start[d]);
        start[d] = NONE;
        alias[d] = r as u32;
    }
}

/// Fills `an.weight` once the loops are known.
fn weigh(lir: &Lir, an: &mut Analysis) {
    let n_pos = 2 * an.code.len() + 2;
    let Analysis {
        code,
        weight,
        loops,
        depth,
        start,
        alias,
        ..
    } = an;
    reset(weight, start.len(), 0);
    reset(depth, n_pos + 1, 0);
    for &(h, e) in loops.iter() {
        depth[h as usize] += 1;
        depth[e as usize + 1] -= 1;
    }
    let mut d = 0;
    for x in depth.iter_mut() {
        d += *x;
        *x = d;
    }
    for (k, &ii) in code.iter().enumerate() {
        let p = 2 * k;
        let w = LOOP_WEIGHT.saturating_pow(depth[p] as u32);
        let inst = &lir.insts[ii as usize];
        let mut add = |r: u32| {
            let r = match alias.get(r as usize) {
                Some(&a) if a != NONE => a,
                _ => r,
            };
            weight[r as usize] = weight[r as usize].saturating_add(w);
        };
        for_uses(lir, inst, &mut add);
        for_defs(lir, inst, &mut add);
    }
}
