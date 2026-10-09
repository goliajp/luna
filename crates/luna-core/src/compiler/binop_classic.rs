//! Binary operators of 5.1–5.3, whose operands are `RK` operands: PUC's
//! `luaK_infix` / `luaK_posfix` pass a constant of any type (up to index
//! 255) in the instruction itself, on either side, and a register
//! otherwise.

use super::binop::BinOpOpen;
use super::lvalue::Rk;
use super::*;

impl Compiler<'_> {
    /// [`Compiler::binop_close`] before 5.4.
    pub(super) fn binop_close_classic(
        &mut self,
        op: BinOp,
        le: Exp,
        rhs: ExprId,
        line: u32,
        open: BinOpOpen,
    ) -> Result<Exp, SyntaxError> {
        let (saved, saved_force) = (open.saved, open.saved_force);
        let mut zeros = Vec::new();
        if let Some(folded) = fold_arith(op, &le, self.ast, rhs, self.version, &mut zeros) {
            self.note_zeros(&zeros);
            self.force_line = saved_force;
            return Ok(folded);
        }
        let arith = !matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );
        // `luaK_infix`: the left operand becomes an `RK` operand now, unless
        // it is a numeral of an arithmetic operator, kept for folding and
        // made a constant only after the right operand
        let left = if arith && matches!(le, Exp::Int(_) | Exp::Float(_)) {
            None
        } else {
            let l = self.exp_rk(le)?;
            if let Rk::Reg(r) = l
                && r >= saved
            {
                self.set_freereg(r + 1);
            }
            Some(l)
        };
        self.force_line = saved_force;
        let re = self.expr(rhs)?;
        // `luaK_posfix`: the right operand first
        let r = self.exp_rk(re)?;
        let l = match left {
            Some(l) => l,
            None => self.exp_rk(le)?,
        };
        self.set_freereg(saved);
        // 5.2 / 5.3 put an arithmetic instruction on the operator's line; a
        // comparison, and 5.1's arithmetic, go where the right operand ends
        if !arith {
            return self.compare_rk(op, l, r);
        }
        let saved_force = match self.version {
            LuaVersion::Lua51 => self.force_line,
            _ => self.force_line.replace(line),
        };
        let e = self.arith_rk(op, l, r);
        self.force_line = saved_force;
        e
    }

    /// `R[A] := l op r` in the form its operands take.
    fn arith_rk(&mut self, op: BinOp, l: Rk, r: Rk) -> Result<Exp, SyntaxError> {
        let reg_op = match op {
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::IDiv => Op::IDiv,
            BinOp::Mod => Op::Mod,
            BinOp::Pow => Op::Pow,
            BinOp::BAnd => Op::BAnd,
            BinOp::BOr => Op::BOr,
            BinOp::BXor => Op::BXor,
            BinOp::Shl => Op::Shl,
            _ => Op::Shr,
        };
        let k_op = reg_op.k_form().expect("an arithmetic operator");
        let inst = match (l, r) {
            (Rk::Reg(a), Rk::Reg(b)) => Inst::iabc(reg_op, 0, a, b, false),
            (Rk::Reg(a), Rk::K(k)) => Inst::iabc(k_op, 0, a, k, false),
            (Rk::K(k), Rk::Reg(b)) => Inst::iabc(k_op, 0, b, k, true),
            (Rk::K(x), Rk::K(y)) => {
                let kk = reg_op.kk_form().expect("an arithmetic operator");
                Inst::iabc(kk, 0, x, y, false)
            }
        };
        Ok(Exp::Reloc(self.emit(inst)))
    }

    /// `l op r` as a pending comparison (PUC `codecomp`): `>` and `>=`
    /// compare the operands the other way round.
    fn compare_rk(&mut self, op: BinOp, l: Rk, r: Rk) -> Result<Exp, SyntaxError> {
        let (cmp, l, r) = match op {
            BinOp::Eq | BinOp::Ne => (Op::Eq, l, r),
            BinOp::Lt => (Op::Lt, l, r),
            BinOp::Le => (Op::Le, l, r),
            BinOp::Gt => (Op::Lt, r, l),
            _ => (Op::Le, r, l),
        };
        let (kop, kkop) = match cmp {
            Op::Eq => (Op::EqK, Op::EqKK),
            Op::Lt => (Op::LtK, Op::LtKK),
            _ => (Op::LeK, Op::LeKK),
        };
        let (cop, a, b, c) = match (l, r) {
            (Rk::Reg(a), Rk::Reg(b)) => (cmp, a, b, 0),
            (Rk::Reg(a), Rk::K(k)) => (kop, a, k, 0),
            (Rk::K(k), Rk::Reg(b)) => (kop, b, k, 1),
            (Rk::K(x), Rk::K(y)) => (kkop, x, y, 0),
        };
        self.compare(cop, a, b, c, op != BinOp::Ne)
    }
}
