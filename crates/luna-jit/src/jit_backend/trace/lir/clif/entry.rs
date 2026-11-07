//! The optimizing tier's loop as a function of its own (see
//! `lower::handoff`): the replay starts at `loop_entry`, which first reads
//! from `reg_state` the registers the loop reads before writing them.

use super::*;
use cranelift_codegen::ir::{StackSlotData, StackSlotKind};

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
        let (g, k) = (
            &mut gen_[j * words..][..words],
            &mut kill[j * words..][..words],
        );
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
    let succs = |b: u32| live::replay_succs(lir, an, b);
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
                let x =
                    self.b
                        .ins()
                        .load(types::I64, MemFlagsData::new(), reg_state, (i as i32) * 8);
                self.b.def_var(self.vars[var as usize], x);
            }
        }
    }

    /// Gives the loop `reg_state` as a value of its own, read back from a
    /// stack slot after the registers are read. The argument comes in a
    /// fixed register; the allocator splits its range where the loop
    /// head's code does not use it, and every turn of the loop then moves
    /// it between two registers on the back edge. The value read back has
    /// no fixed register and keeps one through the loop.
    pub(super) fn own_reg_state(&mut self) {
        let reg_state = self.v(self.lir.arg0);
        let slot =
            self.b
                .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        // through memory, not a copy: the allocator would merge a copy back
        // into the argument's range and split it again by the fixed register
        self.b.ins().stack_store(types::I64, reg_state, slot, 0);
        let own = self.b.ins().stack_load(types::I64, types::I64, slot, 0);
        self.vals[self.lir.arg0 as usize] = Some(own);
    }

    /// Notes the variable each value `taken` names was taken from, and the
    /// blocks' immediate dominators in the loop's function.
    pub(super) fn inherit_vars(&mut self, taken: &[(u32, u32)], an: &live::Analysis) {
        self.ivars = vec![None; self.lir.value_ty.len()];
        for &(k, v) in taken {
            self.ivars[v as usize] = Some((self.vars[k as usize], NONE));
        }
        self.idom = idoms(self.lir, an);
    }

    /// Reads each inherited value `i` (in block `blk`) reads from the
    /// variable it was taken from, unless the block its definition was
    /// replayed in dominates `blk`: elsewhere (the loop head's code, which
    /// took it from the back edges, and what only that code reaches) the
    /// variable still holds it, as the lowerer read it in its place.
    pub(super) fn inherited_uses(&mut self, i: &Inst, blk: u32) {
        let nv = self.lir.value_ty.len() as u32;
        let mut uses = Vec::new();
        live::for_uses(self.lir, i, |x| {
            if x < nv && self.ivars[x as usize].is_some() {
                uses.push(x);
            }
        });
        for x in uses {
            let Some((var, at)) = self.ivars[x as usize] else {
                continue;
            };
            if at == NONE || !dominates(&self.idom, at, blk) {
                let v = self.b.use_var(var);
                self.vals[x as usize] = Some(v);
            } else if let Some(d) = self.defs[x as usize] {
                self.vals[x as usize] = Some(d);
            }
        }
    }

    /// Notes where the replay defined an inherited value `i` defines.
    pub(super) fn inherited_def(&mut self, i: &Inst, blk: u32) {
        if i.dst != NONE
            && let Some(Some((_, at))) = self.ivars.get_mut(i.dst as usize)
        {
            *at = blk;
            self.defs[i.dst as usize] = self.vals[i.dst as usize];
        }
    }
}

/// Per block, its immediate dominator among the blocks `an` lays out
/// (`NONE` for the first and the others), the replay's paths: a
/// `TierCount` goes on to its continuation only.
pub(crate) fn idoms(lir: &Lir, an: &live::Analysis) -> Vec<u32> {
    let mut rpo = vec![NONE; lir.blocks.len()];
    for (j, &b) in an.order.iter().enumerate() {
        rpo[b as usize] = j as u32;
    }
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); lir.blocks.len()];
    for &b in &an.order {
        for t in live::replay_succs(lir, an, b)
            .into_iter()
            .filter(|&t| t != NONE && rpo[t as usize] != NONE)
        {
            preds[t as usize].push(b);
        }
    }
    let mut idom = vec![NONE; lir.blocks.len()];
    let Some(&root) = an.order.first() else {
        return idom;
    };
    idom[root as usize] = root;
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &an.order[1..] {
            let mut new = NONE;
            for &p in &preds[b as usize] {
                if idom[p as usize] == NONE {
                    continue;
                }
                new = match new {
                    NONE => p,
                    mut a => {
                        let mut c = p;
                        while a != c {
                            while rpo[a as usize] > rpo[c as usize] {
                                a = idom[a as usize];
                            }
                            while rpo[c as usize] > rpo[a as usize] {
                                c = idom[c as usize];
                            }
                        }
                        a
                    }
                };
            }
            if new != idom[b as usize] {
                idom[b as usize] = new;
                changed = true;
            }
        }
    }
    idom
}

/// Whether block `a` dominates block `b` by `idom`.
pub(crate) fn dominates(idom: &[u32], a: u32, mut b: u32) -> bool {
    loop {
        if a == b {
            return true;
        }
        let up = idom[b as usize];
        if up == NONE || up == b {
            return false;
        }
        b = up;
    }
}
