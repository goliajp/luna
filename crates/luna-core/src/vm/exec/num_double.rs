//! 5.1/5.2 arithmetic on integers. Those dialects have only doubles, so an
//! integer the VM keeps (the result of `#`, `select('#')`, a string length)
//! stands for a double, and an operation on two of them must give what the
//! doubles' own operation gives: the exact result rounded once, `-0` where
//! IEEE gives it, no wrapping.

use crate::runtime::value::Value;

/// The double nearest the exact result `r`, kept as an integer when it is
/// that double's value.
#[inline(always)]
fn rounded(r: i64) -> Value {
    if r.unsigned_abs() <= 1 << 53 {
        return Value::Int(r);
    }
    let f = r as f64;
    // 2^63 rounds up out of range, and the cast back would saturate
    if f < 9_223_372_036_854_775_808.0 && f as i64 == r {
        Value::Int(r)
    } else {
        Value::Float(f)
    }
}

#[inline(always)]
pub(crate) fn add(a: i64, b: i64) -> Value {
    match a.checked_add(b) {
        Some(r) => rounded(r),
        None => Value::Float(a as f64 + b as f64),
    }
}

#[inline(always)]
pub(crate) fn sub(a: i64, b: i64) -> Value {
    match a.checked_sub(b) {
        Some(r) => rounded(r),
        None => Value::Float(a as f64 - b as f64),
    }
}

#[inline(always)]
pub(crate) fn mul(a: i64, b: i64) -> Value {
    match a.checked_mul(b) {
        // zero times a negative number is -0
        Some(0) if (a | b) < 0 => Value::Float(-0.0),
        Some(r) => rounded(r),
        None => Value::Float(a as f64 * b as f64),
    }
}

#[inline(always)]
pub(crate) fn neg(i: i64) -> Value {
    match i {
        0 => Value::Float(-0.0),
        i64::MIN => Value::Float(-(i as f64)),
        i => Value::Int(-i),
    }
}

/// `a - floor(a/b)*b` (PUC `luai_nummod`). Below 2^53 the integer floor
/// modulo is exact and equal to it; a zero divisor gives nan.
#[inline(always)]
pub(crate) fn rem(a: i64, b: i64) -> Value {
    const EXACT: u64 = 1 << 53;
    if b != 0 && a.unsigned_abs() <= EXACT && b.unsigned_abs() <= EXACT {
        Value::Int(super::num::int_mod(a, b))
    } else {
        let (a, b) = (a as f64, b as f64);
        Value::Float(a - (a / b).floor() * b)
    }
}
