//! Live intervals from the laid-out instructions.

use super::*;

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

/// Lists the instructions in layout order and notes, per register, where it
/// is first and last read or written, its weight, and the calls, variable
/// writes and variable reads.
pub(super) fn number(lir: &Lir, an: &mut Analysis, nv: u32) {
    let Analysis {
        order,
        code,
        start,
        end,
        uses,
        calls,
        read,
        depth,
        weight,
        writes,
        var_reads,
        ..
    } = an;
    for &b in order.iter() {
        let mut ii = lir.blocks[b as usize].first;
        while ii != NONE {
            let k = code.len();
            code.push(ii);
            let p = 2 * k as u32;
            let w = LOOP_WEIGHT.saturating_pow(depth[p as usize] as u32);
            let inst = &lir.insts[ii as usize];
            ii = inst.next;
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
    }
}

/// Settles the intervals: the parameter, values nothing reads, variables
/// read from the entry, and registers live across a loop.
pub(super) fn extend(lir: &Lir, an: &mut Analysis, nv: u32) {
    let nreg = (nv as usize) + lir.var_ty.len();
    let Analysis {
        start,
        end,
        uses,
        entry_vars,
        read,
        loops,
        weight,
        ..
    } = an;
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
}

/// A value read from a variable that is not written while the value is
/// live stays in the variable's home: the read is no copy, and the
/// variable, now read wherever the value is, keeps a register more often
/// (it takes the value's weight).
pub(super) fn coalesce_var_reads(an: &mut Analysis) {
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
