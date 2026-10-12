//! Emitting a binary operator on two registers.

use super::*;

impl Compiler<'_> {
    /// Emit `op` on the registers `l` and `r`.
    pub(super) fn emit_binop(&mut self, op: BinOp, l: u32, r: u32) -> Result<Exp, SyntaxError> {
        Ok(match op {
            BinOp::Add => self.arith(Op::Add, l, r),
            BinOp::Sub => self.arith(Op::Sub, l, r),
            BinOp::Mul => self.arith(Op::Mul, l, r),
            BinOp::Div => self.arith(Op::Div, l, r),
            BinOp::IDiv => self.arith(Op::IDiv, l, r),
            BinOp::Mod => self.arith(Op::Mod, l, r),
            BinOp::Pow => self.arith(Op::Pow, l, r),
            BinOp::BAnd => self.arith(Op::BAnd, l, r),
            BinOp::BOr => self.arith(Op::BOr, l, r),
            BinOp::BXor => self.arith(Op::BXor, l, r),
            BinOp::Shl => self.arith(Op::Shl, l, r),
            BinOp::Shr => self.arith(Op::Shr, l, r),
            BinOp::Eq => self.compare(Op::Eq, l, r, 0, true)?,
            BinOp::Ne => self.compare(Op::Eq, l, r, 0, false)?,
            BinOp::Lt => self.compare(Op::Lt, l, r, 0, true)?,
            BinOp::Le => self.compare(Op::Le, l, r, 0, true)?,
            BinOp::Gt => self.compare(Op::Lt, r, l, 0, true)?,
            BinOp::Ge => self.compare(Op::Le, r, l, 0, true)?,
            BinOp::And | BinOp::Or | BinOp::Concat => unreachable!(),
        })
    }

    pub(super) fn arith(&mut self, op: Op, l: u32, r: u32) -> Exp {
        Exp::Reloc(self.emit(Inst::iabc(op, 0, l, r, false)))
    }

    /// PUC `condjump` of a comparison: the test and its jump, taken when
    /// the test gives `k` (PUC `VJMP`).
    pub(super) fn compare(
        &mut self,
        op: Op,
        l: u32,
        r: u32,
        c: u32,
        k: bool,
    ) -> Result<Exp, SyntaxError> {
        let pc = self.cond_jump(Inst::iabc(op, l, r, c, k))?;
        Ok(Exp::Jmp(pc as usize))
    }
}
