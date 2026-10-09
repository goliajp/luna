//! The target-independent half of code generation: frame layout, operand
//! access, block order, branches, parallel moves and calls. Each target
//! implements [`Masm`], the instructions this needs.

use super::alloc::{Allocation, Loc};
use super::live::Analysis;
use super::pmove::{Src, sequence};
use super::*;

pub(crate) use super::masm::*;

mod calls;

pub(crate) struct Gen<'a, M: Masm> {
    pub(crate) lir: &'a Lir,
    pub(crate) an: &'a Analysis,
    pub(crate) al: &'a Allocation,
    pub(crate) m: M,
    labels: Vec<Label>,
    slot_off: Vec<i32>,
    spill_base: i32,
    /// A comparison whose result only feeds the next branch: its flags.
    pub(crate) pending: Option<(u32, Cond)>,
    /// Parallel moves being collected (integer, float), and their order.
    mv: [Vec<(Loc, Src)>; 2],
    seq: Vec<(Loc, Src)>,
}

/// The registers in `set`, in the first `n` entries.
fn bits_of(set: u64) -> ([u8; 64], usize) {
    let mut out = [0u8; 64];
    let mut n = 0;
    for r in 0..64u8 {
        if set & (1 << r) != 0 {
            out[n] = r;
            n += 1;
        }
    }
    (out, n)
}

/// The generator's buffers, kept from one trace to the next.
#[derive(Default)]
pub(crate) struct CgBufs {
    labels: Vec<Label>,
    slot_off: Vec<i32>,
    mv: [Vec<(Loc, Src)>; 2],
    seq: Vec<(Loc, Src)>,
}

/// Generates the function; `Err` names the first primitive the target
/// lacks.
pub(crate) fn generate<M: Masm>(
    lir: &Lir,
    an: &Analysis,
    al: &Allocation,
    m: M,
    bufs: &mut CgBufs,
) -> Result<Bufs, &'static str> {
    // outgoing arguments past the registers go right above the shadow space
    let out_args = an
        .code
        .iter()
        .map(|&ii| &lir.insts[ii as usize])
        .filter_map(|i| match i.op {
            Op::Call => Some(&lir.funcs[i.a as usize]),
            Op::CallIndirect => Some(&lir.sigs[i.b as usize]),
            _ => None,
        })
        .filter_map(|c| Gen::<M>::stack_args(lir.params(c)))
        .max()
        .unwrap_or(0);
    let mut off = M::CALL_SHADOW as i32 + 8 * out_args as i32;
    let mut slot_off = std::mem::take(&mut bufs.slot_off);
    slot_off.clear();
    for &(size, align) in &lir.slots {
        let a = 1i32 << align.max(3);
        off = (off + a - 1) & !(a - 1);
        slot_off.push(off);
        off += size as i32;
    }
    off = (off + 7) & !7;
    let spill_base = off;
    off += 8 * al.spill_slots as i32;
    let locals = ((off + 15) & !15) as u32;
    // spill and slot offsets must fit every target's scaled load offset
    if locals > 32_000 {
        return Err("frame too large");
    }
    let mut g = Gen {
        lir,
        an,
        al,
        m,
        labels: std::mem::take(&mut bufs.labels),
        slot_off,
        spill_base,
        pending: None,
        mv: std::mem::take(&mut bufs.mv),
        seq: std::mem::take(&mut bufs.seq),
    };
    g.labels.clear();
    for _ in 0..lir.blocks.len() {
        let l = g.m.new_label();
        g.labels.push(l);
    }
    let (saved, n) = bits_of(al.callee_used[0]);
    let (fsaved, nf) = bits_of(al.callee_used[1]);
    g.m.prologue(&saved[..n], &fsaved[..nf], locals);
    // reg_state arrives in the first argument register
    if lir.arg0 != NONE {
        let a0 = M::INT_ARGS[0];
        g.write_from(lir.arg0, a0);
    }
    // a variable read before any write reads zero, as with Cranelift
    for &k in &an.entry_vars {
        let r = an.n_values + k;
        if lir.var_ty[k as usize].is_float() {
            g.m.mov_imm(M::SCRATCH[0], 0);
            let d = g.fdst(r, 0);
            g.m.bits_to_f(d, M::SCRATCH[0]);
            g.fcommit(r, d);
        } else {
            let d = g.dst(r, 0);
            g.m.mov_imm(d, 0);
            g.commit(r, d);
        }
    }
    for (k, &b) in an.order.iter().enumerate() {
        let next = an.order.get(k + 1).copied().unwrap_or(NONE);
        // a loop's head starts a 16-byte block, as in the optimizing tier's code
        let at = 2 * an.block_at[b as usize].0;
        if an.loops.iter().any(|&(head, _)| head == at) {
            g.m.align(16);
        }
        g.m.bind(g.labels[b as usize]);
        let (lo, hi) = an.block_at[b as usize];
        for c in lo..hi {
            let ii = an.code[c as usize];
            let peek = an.code.get(c as usize + 1).copied();
            g.inst(ii, peek, next)?;
        }
    }
    let Gen {
        m,
        labels,
        slot_off,
        mv,
        seq,
        ..
    } = g;
    *bufs = CgBufs {
        labels,
        slot_off,
        mv,
        seq,
    };
    Ok(m.finish())
}

impl<M: Masm> Gen<'_, M> {
    pub(crate) fn loc(&self, r: u32) -> Loc {
        self.al.loc[r as usize]
    }

    pub(crate) fn spill_off(&self, s: u32) -> i32 {
        self.spill_base + 8 * s as i32
    }

    pub(crate) fn slot_off(&self, slot: u32) -> i32 {
        self.slot_off[slot as usize]
    }

    /// Integer vreg `r` in a register (reloaded into scratch `k` if spilled).
    pub(crate) fn src(&mut self, r: u32, k: usize) -> u8 {
        match self.loc(r) {
            Loc::Reg(p) => p,
            Loc::Stack(s) => {
                let o = self.spill_off(s);
                self.m.load(Width::B8, M::SCRATCH[k], M::SP, o);
                M::SCRATCH[k]
            }
            Loc::None => M::SCRATCH[k],
        }
    }

    pub(crate) fn fsrc(&mut self, r: u32, k: usize) -> u8 {
        match self.loc(r) {
            Loc::Reg(p) => p,
            Loc::Stack(s) => {
                let o = self.spill_off(s);
                self.m.fload(M::FSCRATCH[k], M::SP, o);
                M::FSCRATCH[k]
            }
            Loc::None => M::FSCRATCH[k],
        }
    }

    /// The register to compute integer vreg `r` into.
    pub(crate) fn dst(&self, r: u32, k: usize) -> u8 {
        match self.loc(r) {
            Loc::Reg(p) => p,
            _ => M::SCRATCH[k],
        }
    }

    pub(crate) fn fdst(&self, r: u32, k: usize) -> u8 {
        match self.loc(r) {
            Loc::Reg(p) => p,
            _ => M::FSCRATCH[k],
        }
    }

    /// Stores `reg` to `r`'s spill slot when `r` lives there.
    pub(crate) fn commit(&mut self, r: u32, reg: u8) {
        if let Loc::Stack(s) = self.loc(r) {
            let o = self.spill_off(s);
            self.m.store(Width::B8, reg, M::SP, o);
        }
    }

    pub(crate) fn fcommit(&mut self, r: u32, reg: u8) {
        if let Loc::Stack(s) = self.loc(r) {
            let o = self.spill_off(s);
            self.m.fstore(reg, M::SP, o);
        }
    }

    /// `r = reg` wherever `r` lives.
    pub(crate) fn write_from(&mut self, r: u32, reg: u8) {
        match self.loc(r) {
            Loc::Reg(p) => self.m.mov(p, reg),
            Loc::Stack(_) => self.commit(r, reg),
            Loc::None => {}
        }
    }

    pub(crate) fn is_float(&self, r: u32) -> bool {
        let nv = self.an.n_values;
        if r < nv {
            self.lir.value_ty[r as usize].is_float()
        } else {
            self.lir.var_ty[(r - nv) as usize].is_float()
        }
    }

    /// Copies vreg `s` into vreg `d`.
    pub(crate) fn copy(&mut self, d: u32, s: u32) {
        let (dl, sl) = (self.loc(d), self.loc(s));
        if dl == sl || dl == Loc::None {
            return;
        }
        let float = self.is_float(s);
        self.one_move(dl, Src::Loc(sl), float);
    }

    /// Performs the moves collected in `self.mv[k]` (`k` 1 for floats) as
    /// one parallel move.
    fn flush_moves(&mut self, k: usize) {
        let float = k == 1;
        let park = if float {
            Loc::Reg(M::FSCRATCH[1])
        } else {
            Loc::Reg(M::SCRATCH[1])
        };
        let mut mv = std::mem::take(&mut self.mv[k]);
        let mut seq = std::mem::take(&mut self.seq);
        sequence(&mut mv, park, &mut seq);
        for &(d, s) in &seq {
            self.one_move(d, s, float);
        }
        mv.clear();
        self.mv[k] = mv;
        self.seq = seq;
    }

    fn one_move(&mut self, d: Loc, s: Src, float: bool) {
        let t = if float { M::FSCRATCH[0] } else { M::SCRATCH[0] };
        let r = match s {
            Src::Loc(Loc::Reg(p)) => p,
            Src::Loc(Loc::Stack(sl)) => {
                let o = self.spill_off(sl);
                if float {
                    self.m.fload(t, M::SP, o);
                } else {
                    self.m.load(Width::B8, t, M::SP, o);
                }
                t
            }
            Src::Loc(Loc::None) => return,
        };
        match d {
            Loc::Reg(p) if float => self.m.fmov(p, r),
            Loc::Reg(p) => self.m.mov(p, r),
            Loc::Stack(sl) => {
                let o = self.spill_off(sl);
                if float {
                    self.m.fstore(r, M::SP, o);
                } else {
                    self.m.store(Width::B8, r, M::SP, o);
                }
            }
            Loc::None => {}
        }
    }

    /// Moves the arguments `args` into block `b`'s parameters.
    fn edge_moves(&mut self, b: u32, args: &[u32]) {
        let lir = self.lir;
        let params = lir.block_params(b);
        if params.is_empty() {
            return;
        }
        for (&p, &a) in params.iter().zip(args) {
            let m = (self.loc(p), Src::Loc(self.loc(a)));
            let k = usize::from(self.is_float(p));
            self.mv[k].push(m);
        }
        self.flush_moves(0);
        self.flush_moves(1);
    }

    pub(crate) fn jump(&mut self, b: u32, args: &[u32], next: u32) {
        self.edge_moves(b, args);
        if b != next {
            self.m.jmp(self.labels[b as usize]);
        }
    }

    /// `brif` on `c` (or on the flags of the comparison fused into it).
    pub(crate) fn brif(&mut self, i: &Inst, n_then: u32, next: u32) {
        let args = &self.lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize];
        let (ta, ea) = args.split_at(n_then as usize);
        let (tb, eb) = (i.b, i.c);
        let cond = self.pending.take().filter(|&(v, _)| v == i.a).map(|p| p.1);
        let plain = ta.is_empty() && ea.is_empty();
        // branch to `target` when the condition is `want`
        let branch = |g: &mut Self, want: bool, target: Label| match cond {
            Some(c) => g.m.jcc(if want { c } else { c.invert() }, target),
            None => {
                let r = g.src(i.a, 0);
                g.m.branch_reg(r, want, target);
            }
        };
        let (tl, el) = (self.labels[tb as usize], self.labels[eb as usize]);
        if plain {
            if tb == next {
                branch(self, false, el);
            } else {
                branch(self, true, tl);
                if eb != next {
                    self.m.jmp(el);
                }
            }
            return;
        }
        let else_edge = self.m.new_label();
        branch(self, false, else_edge);
        self.jump(tb, ta, NONE);
        self.m.bind(else_edge);
        self.jump(eb, ea, next);
    }

    pub(crate) fn tier_count(&mut self, cell: u32, at: u32, hot: u32, cont: u32, next: u32) {
        let [a, n] = M::SCRATCH;
        self.m.mov_reloc(a, self.lir.relocs[cell as usize].1, cell);
        self.m.load(Width::B4, n, a, 0);
        if !self.m.alu_imm(Alu::Add, false, n, n, 1) {
            unreachable!("adding one always encodes");
        }
        self.m.store(Width::B4, n, a, 0);
        if !self.m.cmp_imm(false, n, i64::from(at as i32)) {
            self.m.mov_imm(a, i64::from(at));
            self.m.cmp(false, n, a);
        }
        self.m.jcc(Cond::Eq, self.labels[hot as usize]);
        if cont != next {
            self.m.jmp(self.labels[cont as usize]);
        }
    }
}
