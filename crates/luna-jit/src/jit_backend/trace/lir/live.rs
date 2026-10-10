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

mod operands;
pub(crate) use operands::{for_defs, for_uses, succs};

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
    pub(crate) loops: Vec<(u32, u32)>,
    depth: Vec<i32>,
    writes: Vec<Vec<u32>>,
    /// `(value, variable, position)` of each variable read.
    var_reads: Vec<(u32, u32, u32)>,
}

/// How much more a read or write inside a loop counts than one outside.
const LOOP_WEIGHT: u32 = 8;

fn reset<T: Clone>(v: &mut Vec<T>, n: usize, x: T) {
    v.clear();
    v.resize(n, x);
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
    // the loops, from the back edges: a successor laid out at or before
    // the block
    an.loops.clear();
    for &b in &an.order {
        let (lo, hi) = an.block_at[b as usize];
        if hi > lo {
            for s in succs(lir, b) {
                if s != NONE && an.block_at[s as usize].0 <= lo {
                    an.loops.push((2 * an.block_at[s as usize].0, 2 * hi - 1));
                }
            }
        }
    }
    // per position: how many loops are around it
    let n_pos = 2 * an.code.len() + 2;
    reset(&mut an.depth, n_pos + 1, 0);
    for &(h, e) in &an.loops {
        an.depth[h as usize] += 1;
        an.depth[e as usize + 1] -= 1;
    }
    let mut d = 0;
    for x in an.depth.iter_mut() {
        d += *x;
        *x = d;
    }
    reset(&mut an.start, nreg, NONE);
    reset(&mut an.end, nreg, 0);
    reset(&mut an.uses, nv as usize, 0);
    reset(&mut an.read, lir.var_ty.len(), false);
    reset(&mut an.weight, nreg, 0);
    an.writes.iter_mut().for_each(Vec::clear);
    an.writes.resize_with(lir.var_ty.len(), Vec::new);
    an.var_reads.clear();
    an.calls.clear();
    let Analysis {
        code,
        start,
        end,
        uses,
        calls,
        entry_vars,
        read,
        loops,
        depth,
        weight,
        writes,
        var_reads,
        ..
    } = an;
    #[inline(always)]
    fn touch(start: &mut [u32], end: &mut [u32], weight: &mut [u32], r: u32, p: u32, w: u32) {
        let r = r as usize;
        if start[r] == NONE {
            start[r] = p;
        }
        if p > end[r] {
            end[r] = p;
        }
        weight[r] = weight[r].saturating_add(w);
    }
    for (k, &ii) in code.iter().enumerate() {
        let p = 2 * k as u32;
        let w = LOOP_WEIGHT.saturating_pow(depth[p as usize] as u32);
        let inst = &lir.insts[ii as usize];
        for_uses(lir, inst, |r| {
            if r < nv {
                uses[r as usize] += 1;
            } else {
                read[(r - nv) as usize] = true;
            }
            touch(start, end, weight, r, p, w);
        });
        for_defs(lir, inst, |r| touch(start, end, weight, r, p + 1, w));
        match inst.op {
            Op::Call | Op::CallIndirect => calls.push(p),
            Op::VarWrite => writes[inst.a as usize].push(p + 1),
            Op::VarRead => var_reads.push((inst.dst, inst.a, p + 1)),
            _ => {}
        }
    }
    // the function's parameter arrives in the entry block
    if lir.arg0 != NONE {
        touch(start, end, weight, lir.arg0, 0, 0);
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
    coalesce_var_reads(an);
}

/// A value read from a variable that is not written while the value is
/// live stays in the variable's home: the read is no copy, and the
/// variable, now read wherever the value is, keeps a register more often
/// (it takes the value's weight).
fn coalesce_var_reads(an: &mut Analysis) {
    let nv = an.n_values as usize;
    let Analysis {
        start,
        end,
        alias,
        writes,
        var_reads,
        weight,
        ..
    } = an;
    reset(alias, nv, NONE);
    for &(dst, var, p) in var_reads.iter() {
        let d = dst as usize;
        if start[d] == NONE {
            continue;
        }
        let w = &writes[var as usize];
        // the first write after the read, in layout order: loops extended
        // the value's interval over every write that can run while it lives
        let next = w.partition_point(|&x| x <= p);
        if w.get(next).is_some_and(|&x| x <= end[d]) {
            continue;
        }
        let r = nv + var as usize;
        end[r] = end[r].max(end[d]);
        start[r] = start[r].min(start[d]);
        start[d] = NONE;
        alias[d] = r as u32;
        weight[r] = weight[r].saturating_add(weight[d]);
        weight[d] = 0;
    }
}
