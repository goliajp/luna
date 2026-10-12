//! Constant folding and literal tests over the AST.

use super::Exp;
use super::ctconst::{Arith, fold_numbers};
use super::{Chunk, Expr};
use crate::frontend::ast::{BinOp, ExprId, UnOp};
use crate::numeric::Num;
use crate::version::LuaVersion;

/// Constant-fold arithmetic over two numeric literals where Lua semantics
/// are total (no division-by-zero style runtime errors). `zeros` gets the
/// zero literals of any 5.1 condition skipped on the way (see `numeral`).
pub(super) fn fold_arith(
    op: BinOp,
    le: &Exp,
    ast: &Chunk,
    rhs: ExprId,
    version: LuaVersion,
    zeros: &mut Vec<f64>,
) -> Option<Exp> {
    let l = match le {
        Exp::Int(i) => Num::Int(*i),
        Exp::Float(f) => Num::Float(*f),
        _ => return None,
    };
    let r = numeral(ast, rhs, version, zeros)?;
    Some(match fold_nums(op, l, (ast, rhs, r), version)? {
        Num::Int(i) => Exp::Int(i),
        Num::Float(f) => Exp::Float(f),
    })
}

/// The value of `id` when PUC's parser would have folded it to a numeral:
/// a literal, possibly parenthesized, negated, or combined with another by
/// a folding operator. (The left operand arrives compiled; this reads the
/// right one, which has not been compiled yet.)
///
/// 5.1's parser also drops the jumps of a logical operation whose outcome
/// is fixed, so `C and nil or 2` and `true and 2` are the numeral 2; only a
/// zero's sign makes that observable (`zero_51`). `C` is taken only when it
/// is a literal or an equality test of two, which has nothing to run; the
/// zeros such a test puts in the constant table are pushed on `zeros`.
pub(super) fn numeral(
    ast: &Chunk,
    id: ExprId,
    version: LuaVersion,
    zeros: &mut Vec<f64>,
) -> Option<Num> {
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return None;
    }
    let v51 = version == LuaVersion::Lua51;
    // the left spine of arithmetic operators, outermost first: a long
    // chain (`1 + 1 + ... + 1`) is folded without recursion
    let mut ops: Vec<(BinOp, ExprId)> = Vec::new();
    let mut cur = id;
    while let Expr::BinOp { op, lhs, rhs, .. } = *ast.expr(cur) {
        if v51 && matches!(op, BinOp::And | BinOp::Or) {
            break;
        }
        ops.push((op, rhs));
        cur = lhs;
    }
    let mut z = Vec::new();
    let mut n = numeral_leaf(ast, cur, version, &mut z)?;
    for (op, rhs) in ops.into_iter().rev() {
        let r = numeral(ast, rhs, version, &mut z)?;
        n = fold_nums(op, n, (ast, rhs, r), version)?;
    }
    zeros.extend(z);
    Some(n)
}

/// [`numeral`] of an expression that is not an arithmetic operator.
fn numeral_leaf(ast: &Chunk, id: ExprId, version: LuaVersion, zeros: &mut Vec<f64>) -> Option<Num> {
    let v51 = version == LuaVersion::Lua51;
    match ast.expr(id) {
        Expr::Int(i) => Some(Num::Int(*i)),
        Expr::Float(f) => Some(Num::Float(*f)),
        Expr::Paren(inner) => numeral(ast, *inner, version, zeros),
        Expr::UnOp {
            op: op @ (UnOp::Neg | UnOp::BNot),
            operand,
            ..
        } => fold_unary(*op, numeral(ast, *operand, version, zeros)?, version),
        Expr::BinOp {
            op: BinOp::Or,
            lhs,
            rhs,
            ..
        } if v51 => {
            let mut z = Vec::new();
            if !always_falsy(ast, *lhs, &mut z) {
                return None;
            }
            let n = numeral(ast, *rhs, version, &mut z)?;
            zeros.extend(z);
            Some(n)
        }
        Expr::BinOp {
            op: BinOp::And,
            lhs,
            rhs,
            ..
        } if v51 => {
            if !matches!(literal(ast, *lhs), Some(Lit::Truthy(_))) {
                return None;
            }
            numeral(ast, *rhs, version, zeros)
        }
        _ => None,
    }
}

pub(super) fn is_logical(ast: &Chunk, id: ExprId) -> bool {
    match ast.expr(id) {
        Expr::Paren(inner) => is_logical(ast, *inner),
        Expr::BinOp { op, .. } => matches!(op, BinOp::And | BinOp::Or),
        _ => false,
    }
}

/// A literal operand as 5.1's parser sees it (a negated numeral folds).
enum Lit {
    Falsy,
    /// its number when it is one
    Truthy(Option<f64>),
}

fn literal(ast: &Chunk, id: ExprId) -> Option<Lit> {
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return None;
    }
    Some(match ast.expr(id) {
        Expr::Nil | Expr::False => Lit::Falsy,
        Expr::True | Expr::Str(_) => Lit::Truthy(None),
        Expr::Int(i) => Lit::Truthy(Some(*i as f64)),
        Expr::Float(f) => Lit::Truthy(Some(*f)),
        Expr::Paren(inner) => return literal(ast, *inner),
        Expr::UnOp {
            op: UnOp::Neg,
            operand,
            ..
        } => match literal(ast, *operand)? {
            Lit::Truthy(Some(f)) => Lit::Truthy(Some(-f)),
            _ => return None,
        },
        _ => return None,
    })
}

/// `nil`, `false`, or `C and` one of them where `C` is a literal or an
/// equality test of two literals.
fn always_falsy(ast: &Chunk, id: ExprId, zeros: &mut Vec<f64>) -> bool {
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return false;
    }
    match ast.expr(id) {
        Expr::Paren(inner) => always_falsy(ast, *inner, zeros),
        Expr::BinOp {
            op: BinOp::And,
            lhs,
            rhs,
            ..
        } => fixed_condition(ast, *lhs, zeros) && always_falsy(ast, *rhs, zeros),
        _ => matches!(literal(ast, id), Some(Lit::Falsy)),
    }
}

/// A literal, or an equality test of two whose zeros (left first) go on
/// `zeros`: PUC compiles the test's operands as constants.
fn fixed_condition(ast: &Chunk, id: ExprId, zeros: &mut Vec<f64>) -> bool {
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return false;
    }
    match ast.expr(id) {
        Expr::Paren(inner) => fixed_condition(ast, *inner, zeros),
        Expr::BinOp {
            op: BinOp::Eq | BinOp::Ne,
            lhs,
            rhs,
            ..
        } => {
            let (Some(l), Some(r)) = (literal(ast, *lhs), literal(ast, *rhs)) else {
                return false;
            };
            for lit in [l, r] {
                if let Lit::Truthy(Some(f)) = lit
                    && f == 0.0
                {
                    zeros.push(f);
                }
            }
            true
        }
        _ => literal(ast, id).is_some(),
    }
}

/// `-n` or `~n` as PUC's parser folds it: 5.1 / 5.2 negate any number
/// (they have no `~`); 5.3+ fold through `constfolding`, which leaves a
/// float zero (`-0.0`) and a float without an integer value under `~` be.
pub(super) fn fold_unary(op: UnOp, n: Num, version: LuaVersion) -> Option<Num> {
    if version >= LuaVersion::Lua53 {
        let op = if op == UnOp::Neg {
            Arith::Unm
        } else {
            Arith::BNot
        };
        return fold_numbers(op, n, Num::Int(0), version == LuaVersion::Lua53);
    }
    match (op, n) {
        (UnOp::Neg, Num::Int(i)) => Some(Num::Int(i.wrapping_neg())),
        (UnOp::Neg, Num::Float(f)) => Some(Num::Float(-f)),
        _ => None,
    }
}

/// `l op r`, where `r` is the value of node `rhs` of `ast`.
fn fold_nums(
    op: BinOp,
    l: Num,
    (ast, rhs, r): (&Chunk, ExprId, Num),
    version: LuaVersion,
) -> Option<Num> {
    use Num::*;
    note_fold(ast, op, l, (rhs, r), version);
    if version >= LuaVersion::Lua53 {
        return fold_numbers(Arith::of(op)?, l, r, version == LuaVersion::Lua53);
    }
    // 5.1 / 5.2: PUC leaves a division or modulo by zero to run time
    if matches!(op, BinOp::Div | BinOp::Mod) && r.as_f64() == 0.0 {
        return None;
    }
    let v = match (op, l, r) {
        // they fold `%` and `^` as well (luai_nummod, luai_numpow); where a
        // zero's sign is kept in the constant table, that is observable
        (BinOp::Mod, a, b) => {
            let (a, b) = (a.as_f64(), b.as_f64());
            Float(crate::numeric::nummod_floor(a, b))
        }
        (BinOp::Pow, a, b) => Float(a.as_f64().powf(b.as_f64())),
        (BinOp::Add, Int(a), Int(b)) => Int(a.wrapping_add(b)),
        (BinOp::Sub, Int(a), Int(b)) => Int(a.wrapping_sub(b)),
        (BinOp::Mul, Int(a), Int(b)) => Int(a.wrapping_mul(b)),
        (BinOp::Add, a, b) => Float(a.as_f64() + b.as_f64()),
        (BinOp::Sub, a, b) => Float(a.as_f64() - b.as_f64()),
        (BinOp::Mul, a, b) => Float(a.as_f64() * b.as_f64()),
        (BinOp::Div, a, b) => Float(a.as_f64() / b.as_f64()),
        _ => return None,
    };
    // 5.1's `constfolding` leaves a NaN unfolded; 5.2's folds it
    if let Float(f) = v
        && f.is_nan()
        && version == LuaVersion::Lua51
    {
        return None;
    }
    Some(v)
}

/// Note what PUC's fold of `l op r` leaves in `errno`, `r` being the value
/// of node `rhs`.
pub(super) fn note_fold(
    ast: &Chunk,
    op: BinOp,
    l: Num,
    (rhs, r): (ExprId, Num),
    version: LuaVersion,
) {
    if let Some(stamp) = ast.fold_stamp(rhs) {
        crate::cerrno::fold_effect(rhs.0, stamp, fold_errno(op, l, r, version));
    }
}

/// What PUC's fold of `l op r` leaves in `errno`: its parser calls C `pow`
/// for `^` (5.4 squares instead when `r` is 2), and from 5.3 `fmod` for a
/// float `%` by nonzero, before it decides whether to keep the result.
fn fold_errno(op: BinOp, l: Num, r: Num, version: LuaVersion) -> Option<i32> {
    use crate::cerrno::{Lib, fmod_errno, pow_errno};
    let (a, b) = (l.as_f64(), r.as_f64());
    match op {
        BinOp::Pow if version >= LuaVersion::Lua54 && b == 2.0 => None,
        BinOp::Pow => pow_errno(Lib::HOST, a, b, a.powf(b)),
        BinOp::Mod
            if version >= LuaVersion::Lua53
                && b != 0.0
                && !matches!((l, r), (Num::Int(_), Num::Int(_))) =>
        {
            fmod_errno(a, b)
        }
        _ => None,
    }
}
