//! `==` and `~=` from 5.4 on (PUC `codeeq`): a constant of any type is
//! the instruction's own operand, written on whichever side.

use super::binop::BinOpOpen;
use super::binop_const::Operand;
use super::lvalue::KConst;
use super::*;

/// The left operand once PUC's `luaK_infix` has seen it.
enum Left {
    /// a numeral, kept as it is: it may be an immediate operand
    Num(Exp),
    K(u32),
    Reg(u32),
}

impl Compiler<'_> {
    pub(super) fn binop_eq_modern(
        &mut self,
        op: BinOp,
        le: Exp,
        rhs: ExprId,
        open: BinOpOpen,
    ) -> Result<Exp, SyntaxError> {
        let saved = open.saved;
        let left = match le {
            Exp::Int(_) | Exp::Float(_) => Left::Num(le),
            _ => match self.exp_const(&le) {
                KConst::Fits(c) => Left::K(c),
                _ => {
                    let r = self.exp_to_anyreg(le)?;
                    if r >= saved {
                        self.set_freereg(r + 1);
                    }
                    Left::Reg(r)
                }
            },
        };
        self.force_line = open.saved_force;
        let re = self.expr(rhs)?;
        // the first operand must be in a register: a left one that is not
        // trades places with the right one
        let (r1, form) = match left {
            Left::Reg(r) => {
                let form = self.eq_operand(&re);
                match form {
                    Some(f) => (r, f),
                    None => {
                        let r2 = self.exp_to_anyreg(re)?;
                        (r, Operand::Cmp(Op::Eq, r2, 0))
                    }
                }
            }
            Left::Num(n) => {
                let r1 = self.exp_to_anyreg(re)?;
                let form = match self.eq_operand(&n) {
                    Some(f) => f,
                    None => {
                        if r1 >= saved {
                            self.set_freereg(r1 + 1);
                        }
                        Operand::Cmp(Op::Eq, self.exp_to_anyreg(n)?, 0)
                    }
                };
                (r1, form)
            }
            Left::K(k) => (self.exp_to_anyreg(re)?, Operand::Cmp(Op::EqK, k, 0)),
        };
        self.set_freereg(saved);
        let Operand::Cmp(cop, b, c) = form else {
            unreachable!("an equality operand")
        };
        self.compare(cop, r1, b, c, op == BinOp::Eq)
    }

    /// The second operand of `==` as an immediate (`EqI`) or a constant
    /// (`EqK`), when it is one that fits.
    fn eq_operand(&mut self, e: &Exp) -> Option<Operand> {
        match e {
            Exp::Nil | Exp::True | Exp::False | Exp::Const(_) => match self.exp_const(e) {
                KConst::Fits(c) => Some(Operand::Cmp(Op::EqK, c, 0)),
                _ => None,
            },
            _ => self.const_operand(BinOp::Eq, e, false),
        }
    }
}
