//! The optimizing tier's loop as a function of its own (see
//! `lower::handoff`): the replay starts at `loop_entry`, which first reads
//! from `reg_state` the registers the loop reads before writing them.

use super::*;

/// What the loop's function, starting at `e.block`, reads before writing on
/// some path (the replay's paths: a `TierCount` goes on to its continuation
/// only): per variable, whether it is read so, and the values of
/// `e.inherit` read so, which the loop takes from their variables at the
/// end of `e.block`. `None` when it reads so a value no instruction there
/// defines (but `reg_state` and those of `e.inherit`), or a variable that
/// is not one of the registers `e.regs` names: nothing can set those up.
pub(crate) fn live_in_vars(
    lir: &Lir,
    an: &live::Analysis,
    e: &LoopEntry,
) -> Option<(Vec<bool>, Vec<(u32, u32)>)> {
    let (root, regs) = (e.block, &e.regs[..]);
    let nv = lir.value_ty.len();
    let nreg = nv + lir.var_ty.len();
    let words = nreg.div_ceil(64);
    let set = |s: &mut [u64], k: u32| s[k as usize / 64] |= 1 << (k % 64);
    let has = |s: &[u64], k: u32| s[k as usize / 64] & (1 << (k % 64)) != 0;
    // per laid-out block: what it reads before writing, and what it writes
    let n = an.order.len();
    let mut slot = vec![NONE; lir.blocks.len()];
    let (mut gen_, mut kill) = (vec![0u64; n * words], vec![0u64; n * words]);
    for (j, &b) in an.order.iter().enumerate() {
        slot[b as usize] = j as u32;
        let (g, k) = (&mut gen_[j * words..][..words], &mut kill[j * words..][..words]);
        let (lo, hi) = an.block_at[b as usize];
        for c in lo..hi {
            let i = &lir.insts[an.code[c as usize] as usize];
            live::for_uses(lir, i, |x| {
                if !has(k, x) {
                    set(g, x);
                }
            });
            live::for_defs(lir, i, |x| set(k, x));
        }
    }
    let succs = |b: u32| -> [u32; 2] {
        let last = lir.blocks[b as usize].last;
        match lir.insts.get(last as usize).map(|i| (i.op, i)) {
            Some((Op::TierCount { .. }, i)) => [i.c, NONE],
            _ => live::succs(lir, b),
        }
    };
    let mut live = vec![0u64; n * words];
    let mut out = vec![0u64; words];
    let mut changed = true;
    while changed {
        changed = false;
        for j in (0..n).rev() {
            let b = an.order[j];
            out.fill(0);
            for s in succs(b).into_iter().filter(|&s| s != NONE) {
                let sj = slot[s as usize] as usize;
                for w in 0..words {
                    out[w] |= live[sj * words + w];
                }
            }
            for w in 0..words {
                let x = gen_[j * words + w] | (out[w] & !kill[j * words + w]);
                if x != live[j * words + w] {
                    live[j * words + w] = x;
                    changed = true;
                }
            }
        }
    }
    let rj = slot[root as usize] as usize;
    let (at, root_kill) = (&live[rj * words..][..words], &kill[rj * words..][..words]);
    let mut reads = vec![false; lir.var_ty.len()];
    let mut taken = Vec::new();
    for x in (0..nreg as u32).filter(|&x| has(at, x)) {
        let var = if (x as usize) < nv {
            if x == lir.arg0 {
                continue;
            }
            let &(k, v) = e.inherit.iter().find(|&&(_, v)| v == x)?;
            if !an.code.iter().any(|&c| lir.insts[c as usize].dst == v) {
                return None;
            }
            taken.push((k, v));
            // read at the end of `root`, after what it writes
            if has(root_kill, (nv as u32) + k) {
                continue;
            }
            k
        } else {
            x - nv as u32
        };
        if !regs.contains(&var) {
            return None;
        }
        reads[var as usize] = true;
    }
    Some((reads, taken))
}

impl Replay<'_, '_> {
    /// Reads each register `reads` marks from `reg_state`, at the start of
    /// the loop's function.
    pub(super) fn read_regs(&mut self, regs: &[u32], reads: &[bool]) {
        let reg_state = self.v(self.lir.arg0);
        for (i, &var) in regs.iter().enumerate() {
            if reads[var as usize] {
                let x = self
                    .b
                    .ins()
                    .load(types::I64, MemFlagsData::new(), reg_state, (i as i32) * 8);
                self.b.def_var(self.vars[var as usize], x);
            }
        }
    }

    /// Gives each value `taken` names a variable of its own.
    pub(super) fn inherit_vars(&mut self, taken: &[(u32, u32)]) {
        self.ivars = vec![None; self.lir.value_ty.len()];
        for &(_, v) in taken {
            let t = cty(self.lir.value_ty[v as usize]);
            self.ivars[v as usize] = Some(self.b.declare_var(t));
        }
    }

    /// At the end of `loop_entry`: each inherited value from the variable
    /// the loop head took it from.
    pub(super) fn take_inherited(&mut self, taken: &[(u32, u32)]) {
        for &(k, v) in taken {
            let x = self.b.use_var(self.vars[k as usize]);
            let iv = self.ivars[v as usize].expect("declared");
            self.b.def_var(iv, x);
        }
    }

    /// Reads the inherited values `i` reads from their variables.
    pub(super) fn inherited_uses(&mut self, i: &Inst) {
        let nv = self.lir.value_ty.len() as u32;
        let mut uses: [u32; 8] = [NONE; 8];
        let mut n = 0;
        let mut more = Vec::new();
        live::for_uses(self.lir, i, |x| {
            if x < nv && self.ivars[x as usize].is_some() {
                if n < 8 {
                    uses[n] = x;
                    n += 1;
                } else {
                    more.push(x);
                }
            }
        });
        for x in uses[..n].iter().chain(&more) {
            let iv = self.ivars[*x as usize].expect("declared");
            let v = self.b.use_var(iv);
            self.vals[*x as usize] = Some(v);
        }
    }

    /// Writes an inherited value `i` defines to its variable.
    pub(super) fn inherited_def(&mut self, i: &Inst) {
        if i.dst != NONE
            && let Some(iv) = self.ivars[i.dst as usize]
        {
            let v = self.v(i.dst);
            self.b.def_var(iv, v);
        }
    }
}
