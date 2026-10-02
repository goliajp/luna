//! Linear-scan register allocation over the intervals of [`super::live`].
//!
//! An interval that spans a call only gets a callee-saved register (or the
//! stack); the others prefer caller-saved ones. When no register is free, the
//! interval ending last is spilled for its whole life.

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

pub(crate) struct Allocation {
    pub(crate) loc: Vec<Loc>,
    pub(crate) spill_slots: u32,
    /// Callee-saved registers used, per class (bit per register number).
    pub(crate) callee_used: [u64; 2],
}

fn is_callee(c: &Class, r: u8) -> bool {
    c.callee.contains(&r)
}

pub(crate) fn allocate(lir: &Lir, an: &Analysis, classes: [&Class; 2]) -> Allocation {
    let nv = an.n_values as usize;
    let nreg = an.start.len();
    let float_of = |r: usize| {
        if r < nv {
            lir.value_ty[r].is_float()
        } else {
            lir.var_ty[r - nv].is_float()
        }
    };
    let mut order: Vec<u32> = (0..nreg as u32)
        .filter(|&r| an.start[r as usize] != NONE)
        .collect();
    order.sort_unstable_by_key(|&r| an.start[r as usize]);
    let mut loc = vec![Loc::None; nreg];
    let mut free = [0u64; 2];
    for (k, c) in classes.iter().enumerate() {
        for &r in c.caller.iter().chain(c.callee) {
            free[k] |= 1 << r;
        }
    }
    let mut active: [Vec<u32>; 2] = [Vec::new(), Vec::new()];
    let mut spill_slots = 0u32;
    let mut callee_used = [0u64; 2];
    for &r in &order {
        let ri = r as usize;
        let k = usize::from(float_of(ri));
        let class = classes[k];
        let s = an.start[ri];
        active[k].retain(|&o| {
            if an.end[o as usize] < s {
                if let Loc::Reg(p) = loc[o as usize] {
                    free[k] |= 1 << p;
                }
                false
            } else {
                true
            }
        });
        let crosses = an.crosses_call(r);
        let pick = |set: &[u8]| set.iter().copied().find(|&p| free[k] & (1 << p) != 0);
        let got = if crosses {
            pick(class.callee)
        } else {
            pick(class.caller).or_else(|| pick(class.callee))
        };
        if let Some(p) = got {
            free[k] &= !(1 << p);
            loc[ri] = Loc::Reg(p);
            if is_callee(class, p) {
                callee_used[k] |= 1 << p;
            }
            active[k].push(r);
            continue;
        }
        // spill whichever ends last: an active interval whose register this
        // one may take, or this one
        let victim = active[k]
            .iter()
            .copied()
            .filter(|&o| match loc[o as usize] {
                Loc::Reg(p) => !crosses || is_callee(class, p),
                _ => false,
            })
            .max_by_key(|&o| an.end[o as usize]);
        match victim {
            Some(o) if an.end[o as usize] > an.end[ri] => {
                loc[ri] = loc[o as usize];
                loc[o as usize] = Loc::Stack(spill_slots);
                spill_slots += 1;
                active[k].retain(|&x| x != o);
                active[k].push(r);
            }
            _ => {
                loc[ri] = Loc::Stack(spill_slots);
                spill_slots += 1;
            }
        }
    }
    Allocation {
        loc,
        spill_slots,
        callee_used,
    }
}
