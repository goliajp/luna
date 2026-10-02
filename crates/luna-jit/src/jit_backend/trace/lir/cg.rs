//! The target-independent half of code generation: frame layout, operand
//! access, block order, branches, parallel moves and calls. Each target
//! implements [`Masm`], the instructions this needs.

use super::alloc::{Allocation, Class, Loc};
use super::live::Analysis;
use super::pmove::{Src, sequence};
use super::*;

/// An integer condition, after `cmp a, b`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cond {
    Eq,
    Ne,
    Slt,
    Sle,
    Sgt,
    Sge,
    Ult,
    Ule,
    Ugt,
    Uge,
}

impl Cond {
    pub(crate) fn of(cc: IntCC) -> Option<Cond> {
        Some(match cc {
            IntCC::Equal => Cond::Eq,
            IntCC::NotEqual => Cond::Ne,
            IntCC::SignedLessThan => Cond::Slt,
            IntCC::SignedLessThanOrEqual => Cond::Sle,
            IntCC::SignedGreaterThan => Cond::Sgt,
            IntCC::SignedGreaterThanOrEqual => Cond::Sge,
            IntCC::UnsignedLessThan => Cond::Ult,
            IntCC::UnsignedLessThanOrEqual => Cond::Ule,
            IntCC::UnsignedGreaterThan => Cond::Ugt,
            IntCC::UnsignedGreaterThanOrEqual => Cond::Uge,
        })
    }
    pub(crate) fn invert(self) -> Cond {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Slt => Cond::Sge,
            Cond::Sle => Cond::Sgt,
            Cond::Sgt => Cond::Sle,
            Cond::Sge => Cond::Slt,
            Cond::Ult => Cond::Uge,
            Cond::Ule => Cond::Ugt,
            Cond::Ugt => Cond::Ule,
            Cond::Uge => Cond::Ult,
        }
    }
    pub(crate) fn signed(self) -> bool {
        matches!(self, Cond::Slt | Cond::Sle | Cond::Sgt | Cond::Sge)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Alu {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Lshr,
    Ashr,
    Sdiv,
    Udiv,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Width {
    B1,
    B2,
    B4,
    B8,
}

impl Width {
    pub(crate) fn of(t: Ty) -> Width {
        match t {
            Ty::I8 => Width::B1,
            Ty::I16 => Width::B2,
            Ty::I32 => Width::B4,
            Ty::I64 | Ty::F64 => Width::B8,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Label(pub(crate) u32);

/// The instructions code generation needs from a target. Registers are the
/// target's encoding numbers; integer and floating-point registers are
/// separate spaces. 32-bit (`wide == false`) results are zero-extended.
pub(crate) trait Masm {
    /// The stack pointer as a base register.
    const SP: u8;
    const INT: Class;
    const FLT: Class;
    /// Never allocated: operand reloads and move cycles.
    const SCRATCH: [u8; 2];
    const FSCRATCH: [u8; 2];
    /// Holds an indirect call's target while the arguments are moved.
    const CALL_TARGET: u8;
    const INT_ARGS: &'static [u8];
    const FLOAT_ARGS: &'static [u8];
    const RET: u8;
    const FRET: u8;
    /// Bytes at the stack pointer a callee may use (Win64's shadow space).
    const CALL_SHADOW: u32;
    /// Argument `k` goes in integer or float register `k` (Win64), rather
    /// than in the next free register of its class.
    const POSITIONAL_ARGS: bool;

    fn new_label(&mut self) -> Label;
    fn bind(&mut self, l: Label);
    fn jmp(&mut self, l: Label);
    fn jcc(&mut self, c: Cond, l: Label);
    /// Branch to `l` when `r` is non-zero (`nz`) or zero.
    fn branch_reg(&mut self, r: u8, nz: bool, l: Label);

    fn mov(&mut self, d: u8, s: u8);
    fn mov_imm(&mut self, d: u8, v: i64);
    fn fmov(&mut self, d: u8, s: u8);
    fn bits_to_f(&mut self, d: u8, s: u8);
    fn bits_to_i(&mut self, d: u8, s: u8);
    /// Zero-extending load.
    fn load(&mut self, w: Width, d: u8, base: u8, off: i32);
    fn store(&mut self, w: Width, s: u8, base: u8, off: i32);
    fn fload(&mut self, d: u8, base: u8, off: i32);
    fn fstore(&mut self, s: u8, base: u8, off: i32);
    fn lea(&mut self, d: u8, base: u8, off: i32);

    fn alu(&mut self, op: Alu, wide: bool, d: u8, a: u8, b: u8);
    /// `false` when `imm` has no encoding for `op` (nothing emitted).
    fn alu_imm(&mut self, op: Alu, wide: bool, d: u8, a: u8, imm: i64) -> bool;
    fn neg(&mut self, wide: bool, d: u8, a: u8);
    fn not(&mut self, wide: bool, d: u8, a: u8);
    fn cmp(&mut self, wide: bool, a: u8, b: u8);
    fn cmp_imm(&mut self, wide: bool, a: u8, imm: i64) -> bool;
    fn setcc(&mut self, d: u8, c: Cond);
    /// `d = c ? a : b`
    fn csel(&mut self, d: u8, c: Cond, a: u8, b: u8);
    fn zext(&mut self, d: u8, a: u8, bits: u32);
    fn sext(&mut self, d: u8, a: u8, bits: u32);

    fn fbin(&mut self, op: BinOp, d: u8, a: u8, b: u8);
    fn fneg(&mut self, d: u8, a: u8);
    /// `false` when the target cannot round in place (nothing emitted).
    fn fround(&mut self, d: u8, a: u8, up: bool) -> bool;
    fn i2f(&mut self, d: u8, a: u8);
    fn f2i(&mut self, d: u8, a: u8);
    /// NaN to 0, out of range to the nearest bound.
    fn f2i_sat(&mut self, d: u8, a: u8);
    fn fcmp_set(&mut self, d: u8, cc: FloatCC, a: u8, b: u8) -> bool;

    fn call_abs(&mut self, addr: usize);
    fn call_reg(&mut self, r: u8);
    /// Saves `saved` / `fsaved` and reserves `locals` bytes (a multiple of
    /// 16) at the stack pointer.
    fn prologue(&mut self, saved: &[u8], fsaved: &[u8], locals: u32);
    fn epilogue_ret(&mut self);
    fn finish(self) -> Vec<u8>;
}

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

fn bits_of(set: u64) -> Vec<u8> {
    (0..64u8).filter(|r| set & (1 << r) != 0).collect()
}

/// Generates the function; `Err` names the first primitive the target
/// lacks.
pub(crate) fn generate<M: Masm>(
    lir: &Lir,
    an: &Analysis,
    al: &Allocation,
    m: M,
) -> Result<Vec<u8>, &'static str> {
    let mut off = M::CALL_SHADOW as i32;
    let mut slot_off = Vec::with_capacity(lir.slots.len());
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
        labels: Vec::new(),
        slot_off,
        spill_base,
        pending: None,
        mv: [Vec::new(), Vec::new()],
        seq: Vec::new(),
    };
    g.labels = (0..lir.blocks.len()).map(|_| g.m.new_label()).collect();
    g.m.prologue(
        &bits_of(al.callee_used[0]),
        &bits_of(al.callee_used[1]),
        locals,
    );
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
        g.m.bind(g.labels[b as usize]);
        let (lo, hi) = an.block_at[b as usize];
        for c in lo..hi {
            let ii = an.code[c as usize];
            let peek = an.code.get(c as usize + 1).copied();
            g.inst(ii, peek, next)?;
        }
    }
    Ok(g.m.finish())
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
        let params = &lir.blocks[b as usize].params;
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

    /// A call: arguments into the argument registers, the result out.
    pub(crate) fn call(&mut self, i: &Inst, addr: Option<usize>, params: &[Ty]) {
        let args = &self.lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize];
        if addr.is_none() {
            let r = self.src(i.a, 0);
            self.m.mov(M::CALL_TARGET, r);
        }
        let (mut ni, mut nf) = (0, 0);
        for (k, (&a, &t)) in args.iter().zip(params).enumerate() {
            let s = Src::Loc(self.loc(a));
            if M::POSITIONAL_ARGS {
                (ni, nf) = (k, k);
            }
            if t.is_float() {
                self.mv[1].push((Loc::Reg(M::FLOAT_ARGS[nf]), s));
                nf += 1;
            } else {
                self.mv[0].push((Loc::Reg(M::INT_ARGS[ni]), s));
                ni += 1;
            }
        }
        self.flush_moves(0);
        self.flush_moves(1);
        match addr {
            Some(a) => self.m.call_abs(a),
            None => self.m.call_reg(M::CALL_TARGET),
        }
        if i.dst != NONE {
            if self.is_float(i.dst) {
                let d = self.fdst(i.dst, 0);
                self.m.fmov(d, M::FRET);
                self.fcommit(i.dst, d);
            } else {
                self.write_from(i.dst, M::RET);
            }
        }
    }

    /// Whether a call with these parameter types fits the argument registers.
    pub(crate) fn fits(params: &[Ty]) -> bool {
        if M::POSITIONAL_ARGS {
            return params.len() <= M::INT_ARGS.len().min(M::FLOAT_ARGS.len());
        }
        let nf = params.iter().filter(|t| t.is_float()).count();
        nf <= M::FLOAT_ARGS.len() && params.len() - nf <= M::INT_ARGS.len()
    }
}
