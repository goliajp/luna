//! Linear-scan register allocation over the intervals of [`super::live`].
//!
//! An interval that spans a call only gets a callee-saved register (or the
//! stack); the others prefer caller-saved ones. When no register is free, the
//! interval read and written least (counting loops, see `live`) is spilled
//! for its whole life, the one ending last among equals.

use super::live::Analysis;
use super::*;

/// Where a vreg lives.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Loc {
    /// Never live.
    None,
    Reg(u8),
    /// 8-byte spill slot index.
    Stack(u32),
}

/// The allocatable registers of one class.
pub(crate) struct Class {
    pub(crate) caller: &'static [u8],
    pub(crate) callee: &'static [u8],
}

#[derive(Default)]
pub(crate) struct Allocation {
    pub(crate) loc: Vec<Loc>,
    pub(crate) spill_slots: u32,
    /// Callee-saved registers used, per class (bit per register number).
    pub(crate) callee_used: [u64; 2],
    // scratch, kept for the next trace
    first: Vec<u32>,
    starts: Vec<u32>,
}

fn mask(regs: &[u8]) -> u64 {
    regs.iter().fold(0, |m, &r| m | (1 << r))
}

/// The vregs with an interval, ordered by `key` (a counting sort over
/// positions), into `out`.
fn by_position(key: &[u32], live: &[u32], n_pos: usize, first: &mut Vec<u32>, out: &mut Vec<u32>) {
    first.clear();
    first.resize(n_pos + 1, 0);
    for (r, &k) in key.iter().enumerate() {
        if live[r] != NONE {
            first[k as usize + 1] += 1;
        }
    }
    // the running sum stays in a register: summing in place makes every
    // step wait for the store of the one before
    let mut sum = 0;
    for x in first.iter_mut() {
        sum += *x;
        *x = sum;
    }
    out.clear();
    out.resize(first[n_pos] as usize, 0);
    for (r, &k) in key.iter().enumerate() {
        if live[r] != NONE {
            let at = &mut first[k as usize];
            out[*at as usize] = r as u32;
            *at += 1;
        }
    }
}

/// Fills `al` for `lir`, reusing its buffers.
pub(crate) fn allocate(lir: &Lir, an: &Analysis, classes: [&Class; 2], al: &mut Allocation) {
    let nv = an.n_values as usize;
    let nreg = an.start.len();
    let float_of = |r: usize| {
        if r < nv {
            lir.value_ty[r].is_float()
        } else {
            lir.var_ty[r - nv].is_float()
        }
    };
    let n_pos = 2 * an.code.len() + 2;
    let Allocation {
        loc,
        spill_slots,
        callee_used,
        first,
        starts,
    } = al;
    by_position(&an.start, &an.start, n_pos, first, starts);
    loc.clear();
    loc.resize(nreg, Loc::None);
    let callee = [mask(classes[0].callee), mask(classes[1].callee)];
    let caller = [mask(classes[0].caller), mask(classes[1].caller)];
    let mut free = [callee[0] | caller[0], callee[1] | caller[1]];
    // which vreg holds each register
    let mut holder = [[NONE; 64]; 2];
    *spill_slots = 0;
    *callee_used = [0; 2];
    // registers held, per class, and the earliest end among their holders
    // (no later than it: a spilled holder may have left it behind)
    let mut busy = [0u64; 2];
    let mut soonest = [u32::MAX; 2];
    // the first call after the current start; starts only grow
    let mut ci = 0;
    for &r in starts.iter() {
        let ri = r as usize;
        let s = an.start[ri];
        while ci < an.calls.len() && an.calls[ci] <= s {
            ci += 1;
        }
        // the intervals that ended before this one starts give their
        // registers back
        for k in 0..2 {
            if soonest[k] >= s {
                continue;
            }
            soonest[k] = u32::MAX;
            let mut m = busy[k];
            while m != 0 {
                let p = m.trailing_zeros() as usize;
                m &= m - 1;
                let e = an.end[holder[k][p] as usize];
                if e < s {
                    holder[k][p] = NONE;
                    free[k] |= 1 << p;
                    busy[k] &= !(1 << p);
                } else {
                    soonest[k] = soonest[k].min(e);
                }
            }
        }
        let k = usize::from(float_of(ri));
        let crosses = ci < an.calls.len() && an.calls[ci] < an.end[ri];
        let pick = |set: u64| {
            let m = free[k] & set;
            (m != 0).then(|| m.trailing_zeros() as u8)
        };
        let got = if crosses {
            pick(callee[k])
        } else {
            pick(caller[k]).or_else(|| pick(callee[k]))
        };
        let p = match got {
            Some(p) => p,
            None => {
                // spill the cheapest: a holder whose register this interval
                // may take, or this one
                let allowed = if crosses {
                    callee[k]
                } else {
                    callee[k] | caller[k]
                };
                let cost = |r: usize| (an.weight[r], std::cmp::Reverse(an.end[r]));
                let victim = (0..64u8)
                    .filter(|&p| allowed & (1 << p) != 0 && holder[k][p as usize] != NONE)
                    .min_by_key(|&p| cost(holder[k][p as usize] as usize));
                match victim {
                    Some(p) if cost(holder[k][p as usize] as usize) < cost(ri) => {
                        let o = holder[k][p as usize] as usize;
                        loc[o] = Loc::Stack(*spill_slots);
                        *spill_slots += 1;
                        free[k] |= 1 << p;
                        p
                    }
                    _ => {
                        loc[ri] = Loc::Stack(*spill_slots);
                        *spill_slots += 1;
                        continue;
                    }
                }
            }
        };
        free[k] &= !(1 << p);
        busy[k] |= 1 << p;
        soonest[k] = soonest[k].min(an.end[ri]);
        holder[k][p as usize] = r;
        loc[ri] = Loc::Reg(p);
        if callee[k] & (1 << p) != 0 {
            callee_used[k] |= 1 << p;
        }
    }
    for (d, &r) in an.alias.iter().enumerate() {
        if r != NONE {
            loc[d] = loc[r as usize];
        }
    }
}
