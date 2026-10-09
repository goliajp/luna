//! 5.1–5.3 code keeps an arithmetic or comparison operand that is a small
//! number in the instruction (`AddI`, `LtI`, ...), as 5.4 does, though the
//! dialect's own instruction names a constant (`ADD A B RK(C)`). The
//! constant stays in the table where PUC puts it; only the encoding of the
//! one instruction differs, and [`to_k`] undoes [`to_imm`] exactly: the
//! immediate form is taken only for the first constant of that value, which
//! is the one a lookup by value finds again.

use super::{Inst, MAX_SC, MIN_SC, OFFSET_SC, Op};
use crate::runtime::Value;

/// The immediate form of a constant-operand instruction, or `i` itself.
pub fn to_imm(i: Inst, consts: &[Value]) -> Inst {
    let imm = |k: u32| -> Option<(u32, u32)> {
        let v = *consts.get(k as usize)?;
        let (n, float) = immediate(v)?;
        (first(consts, v) == Some(k as usize)).then_some(((n + OFFSET_SC) as u32, float))
    };
    let (a, b, c, k) = (i.a(), i.b(), i.c(), i.k());
    let arith = |op, x: Option<(u32, u32)>| match x {
        Some((n, 0)) => Inst::iabc(op, a, b, n, k),
        _ => i,
    };
    let cmp = |op, x: Option<(u32, u32)>| match x {
        Some((n, float)) => Inst::iabc(op, a, n, float, k),
        None => i,
    };
    match i.op() {
        Op::AddK => arith(Op::AddI, imm(c)),
        // `K - R` has no immediate form, and `SubI` adds the negated
        // immediate, which keeps `-0.0 - 0` from being -0.0
        Op::SubK if !k => arith(Op::SubI, imm(c).filter(|&(n, _)| n != OFFSET_SC as u32)),
        // `c`: the constant is the left operand, `K < R` being `R > K`
        Op::LtK => cmp(if c == 0 { Op::LtI } else { Op::GtI }, imm(b)),
        Op::LeK => cmp(if c == 0 { Op::LeI } else { Op::GeI }, imm(b)),
        Op::EqK if c == 0 => cmp(Op::EqI, imm(b)),
        _ => i,
    }
}

/// The constant-operand form of an instruction [`to_imm`] made, `i` itself
/// for any other; `None` when the constant is not in the table.
pub fn to_k(i: Inst, consts: &[Value]) -> Option<Inst> {
    let (a, b, k) = (i.a(), i.b(), i.k());
    let idx = |n: i32, float: bool| {
        let v = if float {
            Value::Float(f64::from(n))
        } else {
            Value::Int(i64::from(n))
        };
        first(consts, v).map(|x| x as u32)
    };
    let cmp = |op, c| Some(Inst::iabc(op, a, idx(i.sb(), i.c() != 0)?, c, k));
    match i.op() {
        Op::AddI => Some(Inst::iabc(Op::AddK, a, b, idx(i.sc(), false)?, k)),
        Op::SubI => Some(Inst::iabc(Op::SubK, a, b, idx(i.sc(), false)?, k)),
        Op::LtI => cmp(Op::LtK, 0),
        Op::GtI => cmp(Op::LtK, 1),
        Op::LeI => cmp(Op::LeK, 0),
        Op::GeI => cmp(Op::LeK, 1),
        Op::EqI => cmp(Op::EqK, 0),
        _ => Some(i),
    }
}

/// `v` as an immediate and whether it is a float: an integer, or a float
/// with an integer value that is not -0.0 (whose sign the immediate loses).
fn immediate(v: Value) -> Option<(i32, u32)> {
    let range = i64::from(MIN_SC)..=i64::from(MAX_SC);
    match v {
        Value::Int(n) if range.contains(&n) => Some((n as i32, 0)),
        Value::Float(f)
            if f.fract() == 0.0
                && range.contains(&(f as i64))
                && f.to_bits() != (-0.0f64).to_bits() =>
        {
            Some((f as i32, 1))
        }
        _ => None,
    }
}

/// The first constant that is `v`, bit for bit.
fn first(consts: &[Value], v: Value) -> Option<usize> {
    consts.iter().position(|&c| match (c, v) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        _ => false,
    })
}
