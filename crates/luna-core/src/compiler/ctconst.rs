//! Compile-time constants (5.4+). A `local x <const> = e` whose value PUC's
//! parser can compute — a literal, another such constant, or arithmetic
//! `constfolding` accepts — is not a variable at all: it gets no register
//! and no debug entry, and every use is replaced by the value (an inner
//! function sees it without an upvalue). This module decides that value
//! the way `luaK_exp2const` does after PUC parsed `e`.

use super::{Chunk, Expr, Name};
use crate::frontend::ast::{BinOp, ExprId, Sym, UnOp};
use crate::numeric::Num;
use crate::runtime::value::f2i_exact;

/// The value of a compile-time constant.
#[derive(Clone, Debug)]
pub(super) enum CtConst {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// a literal, by its number in the chunk's names
    Str(Sym),
}

impl CtConst {
    fn truthy(&self) -> bool {
        !matches!(self, CtConst::Nil | CtConst::Bool(false))
    }
}

/// The constant `id` stands for, if PUC would treat it as one; `named`
/// answers for names (a compile-time constant in scope, or `None`).
pub(super) fn ct_value(
    ast: &Chunk,
    id: ExprId,
    named: &mut dyn FnMut(&Name) -> Option<CtConst>,
) -> Option<CtConst> {
    match ast.expr(id) {
        Expr::Nil => Some(CtConst::Nil),
        Expr::True => Some(CtConst::Bool(true)),
        Expr::False => Some(CtConst::Bool(false)),
        Expr::Int(i) => Some(CtConst::Int(*i)),
        Expr::Float(f) => Some(CtConst::Float(*f)),
        Expr::Str(s) => Some(CtConst::Str(*s)),
        Expr::Name(n) => named(n),
        Expr::Paren(inner) => ct_value(ast, *inner, named),
        Expr::UnOp { op, operand, .. } => {
            let v = ct_value(ast, *operand, named)?;
            match op {
                // `codenot` turns a constant operand into a boolean
                UnOp::Not => Some(CtConst::Bool(!v.truthy())),
                UnOp::Neg => fold(Arith::Unm, &v, &CtConst::Int(0), false),
                UnOp::BNot => fold(Arith::BNot, &v, &CtConst::Int(0), false),
                UnOp::Len => None,
            }
        }
        Expr::BinOp { op, lhs, rhs, .. } => {
            let l = ct_value(ast, *lhs, named)?;
            let arith = match op {
                // a constant left operand that decides the outcome emits no
                // jump, leaving the right operand as the result
                BinOp::And => {
                    return if l.truthy() {
                        ct_value(ast, *rhs, named)
                    } else {
                        None
                    };
                }
                BinOp::Or => {
                    return if l.truthy() {
                        None
                    } else {
                        ct_value(ast, *rhs, named)
                    };
                }
                op => Arith::of(*op)?,
            };
            let r = ct_value(ast, *rhs, named)?;
            fold(arith, &l, &r, false)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
    Unm,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    BNot,
}

impl Arith {
    /// The arithmetic or bitwise operation `op` is, if any.
    pub(super) fn of(op: BinOp) -> Option<Arith> {
        Some(match op {
            BinOp::Add => Arith::Add,
            BinOp::Sub => Arith::Sub,
            BinOp::Mul => Arith::Mul,
            BinOp::Div => Arith::Div,
            BinOp::IDiv => Arith::IDiv,
            BinOp::Mod => Arith::Mod,
            BinOp::Pow => Arith::Pow,
            BinOp::BAnd => Arith::BAnd,
            BinOp::BOr => Arith::BOr,
            BinOp::BXor => Arith::BXor,
            BinOp::Shl => Arith::Shl,
            BinOp::Shr => Arith::Shr,
            _ => return None,
        })
    }
}

/// PUC 5.3+ `constfolding` of two numbers (the second is `0` for the
/// unary operations); `v53` selects 5.3's `luai_numpow` and float `%`.
pub(super) fn fold_numbers(op: Arith, a: Num, b: Num, v53: bool) -> Option<Num> {
    let ct = |n: Num| match n {
        Num::Int(i) => CtConst::Int(i),
        Num::Float(f) => CtConst::Float(f),
    };
    match fold(op, &ct(a), &ct(b), v53)? {
        CtConst::Int(i) => Some(Num::Int(i)),
        CtConst::Float(f) => Some(Num::Float(f)),
        _ => None,
    }
}

/// PUC `constfolding`: numbers only; no bitwise operation on a value
/// without an integer representation, no division or modulo by zero; a
/// float result that is NaN or zero is left unfolded (so `-0.0` survives).
/// `v53`: 5.3's `pow` (5.4 squares by multiplying) and float `%` (5.3
/// corrects `fmod` when `m*b < 0`, 5.4 when the signs differ).
fn fold(op: Arith, a: &CtConst, b: &CtConst, v53: bool) -> Option<CtConst> {
    let num = |v: &CtConst| match v {
        CtConst::Int(i) => Some((Some(*i), *i as f64)),
        CtConst::Float(f) => Some((None, *f)),
        _ => None,
    };
    let ((ai, af), (bi, bf)) = (num(a)?, num(b)?);
    let int_of = |i: Option<i64>, f: f64| i.or_else(|| f2i_exact(f));
    let res = match op {
        Arith::BAnd | Arith::BOr | Arith::BXor | Arith::Shl | Arith::Shr | Arith::BNot => {
            let (x, y) = (int_of(ai, af)?, int_of(bi, bf)?);
            CtConst::Int(match op {
                Arith::BAnd => x & y,
                Arith::BOr => x | y,
                Arith::BXor => x ^ y,
                Arith::Shl => shift_left(x, y),
                Arith::Shr => shift_left(x, y.wrapping_neg()),
                _ => !x,
            })
        }
        Arith::Div | Arith::IDiv | Arith::Mod if bf == 0.0 => return None,
        _ => match (ai, bi, op) {
            (Some(x), Some(y), Arith::Add) => CtConst::Int(x.wrapping_add(y)),
            (Some(x), Some(y), Arith::Sub) => CtConst::Int(x.wrapping_sub(y)),
            (Some(x), Some(y), Arith::Mul) => CtConst::Int(x.wrapping_mul(y)),
            (Some(x), _, Arith::Unm) => CtConst::Int(x.wrapping_neg()),
            (Some(x), Some(y), Arith::IDiv) => {
                let q = x.wrapping_div(y);
                CtConst::Int(if x.wrapping_rem(y) != 0 && (x ^ y) < 0 {
                    q - 1
                } else {
                    q
                })
            }
            (Some(x), Some(y), Arith::Mod) => {
                let m = x.wrapping_rem(y);
                CtConst::Int(if m != 0 && (m ^ y) < 0 { m + y } else { m })
            }
            _ => {
                let f = match op {
                    Arith::Add => af + bf,
                    Arith::Sub => af - bf,
                    Arith::Mul => af * bf,
                    Arith::Div => af / bf,
                    Arith::Pow if bf == 2.0 && !v53 => af * af,
                    Arith::Pow => af.powf(bf),
                    Arith::IDiv => (af / bf).floor(),
                    Arith::Mod => {
                        let m = af % bf;
                        let fix = if v53 {
                            m * bf < 0.0
                        } else {
                            (m > 0.0 && bf < 0.0) || (m < 0.0 && bf > 0.0)
                        };
                        if fix { m + bf } else { m }
                    }
                    _ => -af,
                };
                if f.is_nan() || f == 0.0 {
                    return None;
                }
                CtConst::Float(f)
            }
        },
    };
    Some(res)
}

/// `luaV_shiftl`: logical, a negative count shifts right, 64 or more gives 0.
fn shift_left(x: i64, n: i64) -> i64 {
    if n <= -64 || n >= 64 {
        0
    } else if n >= 0 {
        ((x as u64) << n) as i64
    } else {
        ((x as u64) >> -n) as i64
    }
}
