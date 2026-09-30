//! The constant- and immediate-operand half of the 5.4/5.5 encoder (see
//! [`super::modern`]).
//!
//! PUC's forms are `ADDI`, `ADDK`…`BXORK`, `SHRI` and `SHLI`, each
//! followed by the `MMBINI` / `MMBINK` that records the operator, the
//! operand as written and whether it was the left one. PUC's parser
//! subtracts and shifts left by negating the immediate (`x - 1` is
//! `ADDI x -1`, `x << 1` is `SHRI x -1`); the one immediate it cannot
//! negate, 128, goes through the constant table or a scratch register as
//! that parser would put it.

use super::asm::{L, Res};
use super::modern::{M, event};
use crate::runtime::Value;
use crate::vm::dump::puc::modern::Kind;
use crate::vm::isa::{MAX_SC, MIN_SC, OFFSET_SC, Op};

fn fits_sc(i: i32) -> bool {
    (MIN_SC..=MAX_SC).contains(&i)
}

fn enc(i: i32) -> u32 {
    (i + OFFSET_SC) as u32
}

impl M<'_, '_> {
    /// The `K` form of `op`: `lopcodes.h` lists `ADDK`…`BXORK` in a row.
    fn k_opcode(&self, op: Op) -> u32 {
        self.op(Kind::ArithK)
            + match op {
                Op::Add => 0,
                Op::Sub => 1,
                Op::Mul => 2,
                Op::Mod => 3,
                Op::Pow => 4,
                Op::Div => 5,
                Op::IDiv => 6,
                Op::BAnd => 7,
                Op::BOr => 8,
                _ => 9,
            }
    }

    /// `SHRI` / `SHLI`, the two after `BXORK`: 5.4 lists `SHRI` first,
    /// 5.5 `SHLI`.
    fn shift_opcode(&self, shl: bool) -> u32 {
        self.op(Kind::ArithK) + 10 + (shl != self.f.v55) as u32
    }

    /// `R[A] := R[B] op sC`, then the `MMBINI` naming the operator and
    /// the immediate as written (`k`: it was the left operand).
    pub(super) fn arith_i(&mut self, l: L) -> Res<()> {
        let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
        let op = l.op.arith_const_op().expect("immediate arithmetic");
        let c = l.c as i32 - OFFSET_SC;
        let (word, imm) = match op {
            Op::Add => (self.op(Kind::ArithI), c),
            Op::Shr => (self.shift_opcode(false), c),
            Op::Sub if fits_sc(-c) => (self.op(Kind::ArithI), -c),
            Op::Shl if fits_sc(-c) => (self.shift_opcode(false), -c),
            _ => return self.arith_via_register(a, b, op, c as i64, l.k),
        };
        let tm = event(op).expect("arithmetic op");
        self.emit(self.raw_abc(word, a, b, enc(imm), false))?;
        self.emit(self.abc(Kind::MmBinI, b, l.c, tm, l.k))
    }

    /// `R[A] := R[B] op K[C]`, then its `MMBINK`.
    pub(super) fn arith_k(&mut self, l: L) -> Res<()> {
        let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
        let op = l.op.arith_const_op().expect("constant arithmetic");
        let tm = event(op).expect("arithmetic op");
        self.emit(self.raw_abc(self.k_opcode(op), a, b, l.c, false))?;
        self.emit(self.abc(Kind::MmBinK, b, l.c, tm, l.k))
    }

    /// An immediate `ADDI` / `SHRI` cannot negate: the constant table when
    /// the operator has a `K` form and the index fits, else a scratch
    /// register and the register form.
    fn arith_via_register(&mut self, a: u32, b: u32, op: Op, i: i64, flip: bool) -> Res<()> {
        let tm = event(op).expect("arithmetic op");
        if !matches!(op, Op::Shl | Op::Shr) {
            let k = self.asm.konst(Value::Int(i));
            if k <= 255 {
                self.emit(self.raw_abc(self.k_opcode(op), a, b, k, false))?;
                return self.emit(self.abc(Kind::MmBinK, b, k, tm, flip));
            }
        }
        let t = self.asm.temp()?;
        self.emit(self.asbx(Kind::LoadI, t, i as i32))?;
        self.emit(self.abc(Kind::Arith(op), a, b, t, false))?;
        // MMBIN has no flip bit: a left-hand constant swaps its operands
        let (x, y) = if flip { (t, b) } else { (b, t) };
        self.emit(self.abc(Kind::MmBin, x, y, tm, false))
    }

    /// `if ((R[A] cmp sB) ~= k) then pc++`; `C` says the immediate is a
    /// float.
    pub(super) fn cmp_i(&mut self, l: L) -> Res<()> {
        let kind = match l.op {
            Op::EqI => Kind::EqI,
            Op::LtI => Kind::LtI,
            Op::LeI => Kind::LeI,
            Op::GtI => Kind::GtI,
            _ => Kind::GeI,
        };
        let a = self.asm.r(l.a)?;
        self.emit(self.abc(kind, a, l.b, (l.c != 0) as u32, l.k))
    }
}
