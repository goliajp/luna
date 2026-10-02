//! Block layout, liveness and live intervals.
//!
//! Values and variable homes share one virtual-register space: value `n` is
//! vreg `n`, variable `k` is vreg `n_values + k`. Each instruction `k` in
//! layout order reads its operands at position `2k` and writes its result at
//! `2k + 1`; a block's parameters are written by the branches into it. Every
//! vreg gets one interval, the hull of the positions where it is live: an
//! over-approximation that keeps allocation a single linear scan.

use super::*;

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
    /// Variables live into the entry block (read before any write).
    pub(crate) entry_vars: Vec<u32>,
}

/// Calls `f` with each vreg `inst` reads.
pub(crate) fn for_uses(lir: &Lir, i: &Inst, mut f: impl FnMut(u32)) {
    let nv = lir.value_ty.len() as u32;
    let args = &lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize];
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
        Op::Jump | Op::Call => args.iter().for_each(|&x| f(x)),
        Op::Brif(_) | Op::CallIndirect => {
            f(i.a);
            args.iter().for_each(|&x| f(x));
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
        Op::Jump => lir.blocks[i.a as usize].params.iter().for_each(|&p| f(p)),
        Op::Brif(_) => {
            lir.blocks[i.b as usize].params.iter().for_each(|&p| f(p));
            lir.blocks[i.c as usize].params.iter().for_each(|&p| f(p));
        }
        _ => {}
    }
}

fn succs(lir: &Lir, b: u32) -> (u32, u32) {
    let last = lir.blocks[b as usize].last;
    if last == NONE {
        return (NONE, NONE);
    }
    let i = &lir.insts[last as usize];
    match i.op {
        Op::Jump => (i.a, NONE),
        Op::Brif(_) => (i.b, i.c),
        _ => (NONE, NONE),
    }
}

struct Bits {
    words: usize,
    data: Vec<u64>,
}

impl Bits {
    fn new(rows: usize, bits: usize) -> Bits {
        let words = bits.div_ceil(64);
        Bits {
            words,
            data: vec![0; rows * words],
        }
    }
    fn row(&self, r: usize) -> &[u64] {
        &self.data[r * self.words..(r + 1) * self.words]
    }
    fn set(&mut self, r: usize, b: u32) {
        self.data[r * self.words + (b / 64) as usize] |= 1 << (b % 64);
    }
    fn get(&self, r: usize, b: u32) -> bool {
        self.data[r * self.words + (b / 64) as usize] & (1 << (b % 64)) != 0
    }
}

fn layout(lir: &Lir) -> Vec<u32> {
    let n = lir.blocks.len();
    let mut seen = vec![false; n];
    let mut stack = vec![0u32];
    seen[0] = true;
    while let Some(b) = stack.pop() {
        let (s1, s2) = succs(lir, b);
        for s in [s1, s2] {
            if s != NONE && !seen[s as usize] {
                seen[s as usize] = true;
                stack.push(s);
            }
        }
    }
    (0..n as u32).filter(|&b| seen[b as usize]).collect()
}

pub(crate) fn analyze(lir: &Lir) -> Analysis {
    let nv = lir.value_ty.len() as u32;
    let nreg = (nv as usize) + lir.var_ty.len();
    let order = layout(lir);
    let mut code = Vec::with_capacity(lir.insts.len());
    let mut block_at = vec![(NONE, NONE); lir.blocks.len()];
    for &b in &order {
        let first = code.len() as u32;
        let mut i = lir.blocks[b as usize].first;
        while i != NONE {
            code.push(i);
            i = lir.insts[i as usize].next;
        }
        block_at[b as usize] = (first, code.len() as u32);
    }

    // per laid-out block (by index in `order`): uses before defs, defs
    let nb = order.len();
    let mut slot_of = vec![NONE; lir.blocks.len()];
    for (k, &b) in order.iter().enumerate() {
        slot_of[b as usize] = k as u32;
    }
    let mut gen_set = Bits::new(nb, nreg);
    let mut kill = Bits::new(nb, nreg);
    let mut uses = vec![0u32; nv as usize];
    for (k, &b) in order.iter().enumerate() {
        let (lo, hi) = block_at[b as usize];
        for &ii in &code[lo as usize..hi as usize] {
            let inst = &lir.insts[ii as usize];
            for_uses(lir, inst, |r| {
                if r < nv {
                    uses[r as usize] += 1;
                }
                if !kill.get(k, r) {
                    gen_set.set(k, r);
                }
            });
            for_defs(lir, inst, |r| kill.set(k, r));
        }
    }
    let mut live_in = Bits::new(nb, nreg);
    let mut live_out = Bits::new(nb, nreg);
    let w = live_in.words;
    let mut changed = true;
    while changed {
        changed = false;
        for k in (0..nb).rev() {
            let (s1, s2) = succs(lir, order[k]);
            for wi in 0..w {
                let mut out = 0u64;
                for s in [s1, s2] {
                    if s != NONE {
                        out |= live_in.row(slot_of[s as usize] as usize)[wi];
                    }
                }
                let inn = gen_set.row(k)[wi] | (out & !kill.row(k)[wi]);
                if out != live_out.data[k * w + wi] || inn != live_in.data[k * w + wi] {
                    changed = true;
                    live_out.data[k * w + wi] = out;
                    live_in.data[k * w + wi] = inn;
                }
            }
        }
    }

    let mut start = vec![NONE; nreg];
    let mut end = vec![0u32; nreg];
    let mut touch = |r: u32, p: u32| {
        let r = r as usize;
        if start[r] == NONE || p < start[r] {
            start[r] = p;
        }
        if p > end[r] {
            end[r] = p;
        }
    };
    let mut calls = Vec::new();
    for (k, &b) in order.iter().enumerate() {
        let (lo, hi) = block_at[b as usize];
        for r in 0..nreg as u32 {
            if live_in.get(k, r) {
                touch(r, 2 * lo);
            }
            if live_out.get(k, r) {
                touch(r, 2 * hi.max(lo + 1) - 1);
            }
        }
        for (off, &ii) in code[lo as usize..hi as usize].iter().enumerate() {
            let p = 2 * (lo + off as u32);
            let inst = &lir.insts[ii as usize];
            for_uses(lir, inst, |r| touch(r, p));
            for_defs(lir, inst, |r| touch(r, p + 1));
            if matches!(inst.op, Op::Call | Op::CallIndirect) {
                calls.push(p);
            }
        }
    }
    // the function's parameter arrives in the entry block
    if lir.arg0 != NONE {
        touch(lir.arg0, 0);
    }
    let entry_vars = (nv..nreg as u32)
        .filter(|&r| live_in.get(0, r))
        .map(|r| r - nv)
        .collect();
    Analysis {
        order,
        code,
        block_at,
        n_values: nv,
        start,
        end,
        uses,
        calls,
        entry_vars,
    }
}

impl Analysis {
    /// Whether vreg `r` is live across a call (so a caller-saved register
    /// would not survive).
    pub(crate) fn crosses_call(&self, r: u32) -> bool {
        let (s, e) = (self.start[r as usize], self.end[r as usize]);
        let k = self.calls.partition_point(|&c| c <= s);
        k < self.calls.len() && self.calls[k] < e
    }
}
