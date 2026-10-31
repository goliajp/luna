//! Compile-time constants (5.4+). A `local x <const> = e` whose value PUC's
//! parser can compute — a literal, another such constant, or arithmetic
//! `constfolding` accepts — is not a variable at all: it gets no register
//! and no debug entry, and every use is replaced by the value (an inner
//! function sees it without an upvalue). This module decides that value
//! the way `luaK_exp2const` does after PUC parsed `e`.

use super::{Chunk, Expr, Name};
use crate::frontend::ast::{BinOp, ExprId, Sym, UnOp};
use crate::numeric::Num;
use crate::runtime::mem::LVec;
use crate::runtime::value::f2i_exact;
use crate::version::LuaVersion;

mod arith;
use arith::fold;
pub(super) use arith::{Arith, fold_numbers};

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

    pub(super) fn num(&self) -> Option<Num> {
        match *self {
            CtConst::Int(i) => Some(Num::Int(i)),
            CtConst::Float(f) => Some(Num::Float(f)),
            _ => None,
        }
    }
}

/// The constant `id` stands for, if PUC would treat it as one; `named`
/// answers for names (a compile-time constant in scope, or `None`).
pub(super) fn ct_value(
    ast: &Chunk,
    id: ExprId,
    named: &mut dyn FnMut(&Name) -> Option<CtConst>,
) -> Option<CtConst> {
    value(ast, id, named, false)
}

/// [`ct_value`] for an operand that is compiled in full anyway, so code
/// that runs before a constant result is no reason to give it up. Then
/// `X and nil or K` is K whatever X is: PUC compiles X and points its jumps
/// at the `or`, which is left with the constant K and no jumps.
pub(super) fn ct_operand(
    ast: &Chunk,
    id: ExprId,
    named: &mut dyn FnMut(&Name) -> Option<CtConst>,
) -> Option<CtConst> {
    value(ast, id, named, true)
}

/// `X and F` with F always falsy: the value is F, and X only adds jumps.
fn falsy_and(ast: &Chunk, id: ExprId, named: &mut dyn FnMut(&Name) -> Option<CtConst>) -> bool {
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return false;
    }
    match ast.expr(id) {
        Expr::Paren(inner) => falsy_and(ast, *inner, named),
        Expr::BinOp {
            op: BinOp::And,
            rhs,
            ..
        } => {
            falsy_and(ast, *rhs, named)
                || value(ast, *rhs, named, true).is_some_and(|v| !v.truthy())
        }
        _ => false,
    }
}

fn value(
    ast: &Chunk,
    id: ExprId,
    named: &mut dyn FnMut(&Name) -> Option<CtConst>,
    compiled: bool,
) -> Option<CtConst> {
    // a leaf that holds no expression needs no stack check
    if !matches!(
        ast.expr(id),
        Expr::BinOp { .. } | Expr::UnOp { .. } | Expr::Paren(_)
    ) {
        return value_leaf(ast, id, named, compiled);
    }
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return None;
    }
    // the left spine of binary operators, outermost first: a long chain
    // is folded without recursion
    let mut ops = LVec::new(ast.exprs.mem());
    let mut cur = id;
    let mut v = loop {
        match ast.expr(cur) {
            // `X or K` with X compiled in full and always false: K, whatever
            // X is
            Expr::BinOp {
                op: BinOp::Or,
                lhs,
                rhs,
                ..
            } if compiled && falsy_and(ast, *lhs, named) => {
                break value(ast, *rhs, named, compiled)?;
            }
            Expr::BinOp { op, lhs, rhs, .. } => {
                ops.push_or_abort((*op, *rhs));
                cur = *lhs;
            }
            _ => break value_leaf(ast, cur, named, compiled)?,
        }
    };
    for &(op, rhs) in ops.iter().rev() {
        v = match op {
            // a constant left operand that decides the outcome emits no
            // jump, leaving the right operand as the result
            BinOp::And if v.truthy() => value(ast, rhs, named, compiled)?,
            BinOp::Or if !v.truthy() => value(ast, rhs, named, compiled)?,
            BinOp::And | BinOp::Or => return None,
            op => {
                let arith = Arith::of(op)?;
                let r = value(ast, rhs, named, compiled)?;
                if let (Some(a), Some(b)) = (v.num(), r.num()) {
                    super::fold::note_fold(ast, op, a, (rhs, b), LuaVersion::Lua54);
                }
                fold(arith, &v, &r, false)?
            }
        };
    }
    Some(v)
}

/// [`value`] of an expression that is not a binary operator.
fn value_leaf(
    ast: &Chunk,
    id: ExprId,
    named: &mut dyn FnMut(&Name) -> Option<CtConst>,
    compiled: bool,
) -> Option<CtConst> {
    match ast.expr(id) {
        Expr::Nil => Some(CtConst::Nil),
        Expr::True => Some(CtConst::Bool(true)),
        Expr::False => Some(CtConst::Bool(false)),
        Expr::Int(i) => Some(CtConst::Int(*i)),
        Expr::Float(f) => Some(CtConst::Float(*f)),
        Expr::Str(s) => Some(CtConst::Str(*s)),
        Expr::Name(n) => named(n),
        Expr::Paren(inner) => value(ast, *inner, named, compiled),
        Expr::UnOp { op, operand, .. } => {
            let v = value(ast, *operand, named, compiled)?;
            match op {
                // `codenot` turns a constant operand into a boolean
                UnOp::Not => Some(CtConst::Bool(!v.truthy())),
                UnOp::Neg => fold(Arith::Unm, &v, &CtConst::Int(0), false),
                UnOp::BNot => fold(Arith::BNot, &v, &CtConst::Int(0), false),
                UnOp::Len => None,
            }
        }
        _ => None,
    }
}
