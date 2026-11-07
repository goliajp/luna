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

mod intervals;
mod operands;
use intervals::{coalesce_var_reads, extend, number};
use operands::succs_of;
pub(crate) use operands::{Calls, Start, for_defs, for_uses, replay_succs};

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
    /// The function laid out: its `TierCount`s and `Pick`s go on as
    /// [`Start`] says.
    pub(crate) func: Start,
    /// Which of its calling exits go on to the call block.
    pub(crate) exits: Calls,
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

/// Reverse post-order of the blocks reachable from block `root`, into
/// `an.order`.
fn layout(lir: &Lir, an: &mut Analysis, root: u32) {
    let (func, exits) = (an.func, &an.exits);
    let n = lir.blocks.len();
    reset(&mut an.seen, n, false);
    an.order.clear();
    an.stack.clear();
    an.stack.push((root, 0));
    an.seen[root as usize] = true;
    while let Some(top) = an.stack.last_mut() {
        let (b, k) = *top;
        if k == 2 {
            an.order.push(b);
            an.stack.pop();
            continue;
        }
        top.1 += 1;
        let s = succs_of(lir, b, func, exits)[k as usize];
        if s != NONE && !an.seen[s as usize] {
            an.seen[s as usize] = true;
            an.stack.push((s, 0));
        }
    }
    an.order.reverse();
}

/// Fills `an` for `lir`, reusing its buffers.
pub(crate) fn analyze(lir: &Lir, an: &mut Analysis) {
    analyze_from(lir, an, Start::Whole, &Calls::All);
}

/// [`analyze`] for the function `start` names, its calling exits going on
/// as `calls` says.
pub(crate) fn analyze_from(lir: &Lir, an: &mut Analysis, start: Start, calls: &Calls) {
    an.func = start;
    an.exits.clone_from(calls);
    let root = match (start, &lir.loop_entry) {
        (Start::Loop, Some(e)) => e.block,
        _ => 0,
    };
    let nv = lir.value_ty.len() as u32;
    let nreg = (nv as usize) + lir.var_ty.len();
    layout(lir, an, root);
    place_loops(lir, an);
    reset(&mut an.start, nreg, NONE);
    reset(&mut an.end, nreg, 0);
    reset(&mut an.uses, nv as usize, 0);
    reset(&mut an.read, lir.var_ty.len(), false);
    reset(&mut an.weight, nreg, 0);
    an.writes.iter_mut().for_each(Vec::clear);
    an.writes.resize_with(lir.var_ty.len(), Vec::new);
    an.var_reads.clear();
    an.calls.clear();
    number(lir, an, nv);
    extend(lir, an, nv);
    an.n_values = nv;
    coalesce_var_reads(an);
}

/// Each block's place in the layout, the loops (from the back edges: a
/// successor laid out at or before the block) and, per position, how many
/// loops are around it.
fn place_loops(lir: &Lir, an: &mut Analysis) {
    // each block's place in the layout from the blocks' lengths; the
    // instructions themselves are listed in `number`
    reset(&mut an.block_at, lir.blocks.len(), (NONE, NONE));
    let mut at = 0;
    for &b in &an.order {
        let n = lir.blocks[b as usize].len;
        an.block_at[b as usize] = (at, at + n);
        at += n;
    }
    an.code.clear();
    an.code.reserve(at as usize);
    an.loops.clear();
    for &b in &an.order {
        let (lo, hi) = an.block_at[b as usize];
        if hi > lo {
            for s in succs_of(lir, b, an.func, &an.exits) {
                if s != NONE && an.block_at[s as usize].0 <= lo {
                    an.loops.push((2 * an.block_at[s as usize].0, 2 * hi - 1));
                }
            }
        }
    }
    let n_pos = 2 * at as usize + 2;
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
}
