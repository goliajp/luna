//! The constant- and immediate-operand half of the 5.1–5.3 encoder (see
//! [`super::classic`]).
//!
//! These dialects have no immediate forms: the operand is a constant of
//! the dialect's number type named by an `RK` operand (or loaded into a
//! scratch register when its index is past the `RK` range), on the side
//! of the operator the source wrote it. An immediate comparison is the
//! register form on the same `RK`; `GtI` / `GeI` swap the operands of
//! `LT` / `LE`.

use super::asm::{L, Res};
use super::classic::{C, RK_BIT};
use crate::runtime::Value;
use crate::vm::dump::puc::classic::Kind;
use crate::vm::isa::{OFFSET_SC, Op};

impl C<'_, '_> {
    /// An `RK` operand naming constant `k`: the constant itself when its
    /// index fits, else a scratch register it is loaded into.
    pub(super) fn rk(&mut self, k: u32) -> Res<u32> {
        if k < RK_BIT {
            return Ok(k | RK_BIT);
        }
        let t = self.asm.temp()?;
        self.load_k(t, k)?;
        Ok(t)
    }

    pub(super) fn rk_num(&mut self, i: i64) -> Res<u32> {
        let v = self.num(i);
        let k = self.asm.konst(v);
        self.rk(k)
    }

    /// `R(A) := RK(B) op RK(C)` with luna's constant operand as an `RK`.
    pub(super) fn arith_const(&mut self, l: L) -> Res<()> {
        let op = l.op.arith_const_op().expect("constant arithmetic");
        // the register operand folds into an `RK` as in the register forms
        let (a, b) = (self.asm.r(l.a)?, self.operand(l.b)?);
        let c = match l.op {
            Op::AddI | Op::SubI | Op::ShrI | Op::ShlI => {
                self.rk_num((l.c as i32 - OFFSET_SC) as i64)?
            }
            _ => self.rk(l.c)?,
        };
        let (x, y) = if l.k { (c, b) } else { (b, c) };
        self.emit(self.abc(Kind::Arith(op), a, x, y))
    }

    /// `if ((R(A) cmp sB) ~= k) then pc++` as `EQ` / `LT` / `LE` on an
    /// `RK`; the constant is a float when `C` says the literal was one.
    pub(super) fn cmp_const(&mut self, l: L) -> Res<()> {
        let a = self.asm.r(l.a)?;
        let sb = l.b as i32 - OFFSET_SC;
        let v = if l.c != 0 {
            Value::Float(sb as f64)
        } else {
            self.num(sb as i64)
        };
        let k = self.asm.konst(v);
        let imm = self.rk(k)?;
        let (kind, x, y) = match l.op {
            Op::EqI => (Kind::Eq, a, imm),
            Op::LtI => (Kind::Lt, a, imm),
            Op::LeI => (Kind::Le, a, imm),
            Op::GtI => (Kind::Lt, imm, a),
            _ => (Kind::Le, imm, a),
        };
        self.emit(self.abc(kind, l.k as u32, x, y))
    }
}
