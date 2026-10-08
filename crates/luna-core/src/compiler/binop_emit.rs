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
            BinOp::Eq => Exp::Cmp {
                op: Op::Eq,
                l,
                r,
                c: 0,
            },
            BinOp::Ne => self.negate_cmp(Op::Eq, l, r, 0)?,
            BinOp::Lt => Exp::Cmp {
                op: Op::Lt,
                l,
                r,
                c: 0,
            },
            BinOp::Le => Exp::Cmp {
                op: Op::Le,
                l,
                r,
                c: 0,
            },
            BinOp::Gt => Exp::Cmp {
                op: Op::Lt,
                l: r,
                r: l,
                c: 0,
            },
            BinOp::Ge => Exp::Cmp {
                op: Op::Le,
                l: r,
                r: l,
                c: 0,
            },
            BinOp::And | BinOp::Or | BinOp::Concat => unreachable!(),
        })
    }

    pub(super) fn arith(&mut self, op: Op, l: u32, r: u32) -> Exp {
        Exp::Reloc(self.emit(Inst::iabc(op, 0, l, r, false)))
    }

    /// `a ~= b`: comparison materialized with inverted k.
    pub(super) fn negate_cmp(
        &mut self,
        op: Op,
        l: u32,
        r: u32,
        c: u32,
    ) -> Result<Exp, SyntaxError> {
        let reg = self.reserve(1)?;
        self.l().freereg -= 1;
        self.emit(Inst::iabc(op, l, r, c, false));
        self.emit(Inst::isj(Op::Jmp, 1));
        self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
        let tpad = self.here();
        self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
        // Jmp(1) lands on tpad — mark.
        self.mark_target(tpad);
        Ok(Exp::Reg(reg))
    }
}
