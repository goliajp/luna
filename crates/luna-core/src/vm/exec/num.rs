//! Numeric coercion and arithmetic helpers of the interpreter.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArithOp {
    Add,
    Sub,
    Mul,
    Mod,
    Pow,
    Div,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

pub(super) fn as_num(v: Value, version: LuaVersion) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        // PUC forprep coerces numeric strings (`for i = "10", "1", "-2"`).
        Value::Str(s) => str_to_num(s.as_bytes(), version),
        _ => None,
    }
}

/// Float `%` as each dialect's `luai_nummod` defines it. 5.1/5.2 compute
/// `a - floor(a/b)*b` (so `5 % math.huge` is nan); 5.3 fixes `fmod`'s sign
/// when `m*b < 0`; 5.4 compares signs instead, because that product
/// underflows to zero for tiny operands.
pub(super) fn float_mod(version: LuaVersion, a: f64, b: f64) -> f64 {
    if version <= LuaVersion::Lua52 {
        return a - (a / b).floor() * b;
    }
    let m = a % b;
    let fix = if version == LuaVersion::Lua53 {
        m * b < 0.0
    } else {
        (m > 0.0 && b < 0.0) || (m < 0.0 && b > 0.0)
    };
    if fix { m + b } else { m }
}

/// A concatenable operand's byte form (string, or a number coerced to its
/// string), or `None` when only a `__concat` metamethod can handle it.
/// `legacy_float = true` follows PUC ≤5.2's `%.14g` rendering (no `.0`
/// suffix on integer-valued floats) — see `num_to_string_for`.
pub(super) fn concat_piece(v: Value, float_fmt: numeric::FloatFmt) -> Option<Vec<u8>> {
    match v {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Int(x) => Some(numeric::num_to_string(Num::Int(x)).into_bytes()),
        Value::Float(x) => Some(numeric::num_to_string_for(Num::Float(x), float_fmt).into_bytes()),
        _ => None,
    }
}

/// Index into the per-basic-type metatable table for a non-table value
/// (None for tables, which carry their own metatable).
pub(super) fn type_mt_slot(v: Value) -> Option<usize> {
    match v {
        Value::Nil => Some(0),
        Value::Bool(_) => Some(1),
        Value::Int(_) | Value::Float(_) => Some(2),
        Value::Str(_) => Some(3),
        Value::Closure(_) | Value::Native(_) => Some(4),
        // tables and full userdata carry their own metatable; threads and
        // light userdata have none (PUC keeps a shared per-type mt slot for
        // light, but luna doesn't expose it — no test gates on it yet).
        Value::Table(_) | Value::Coro(_) | Value::Userdata(_) | Value::LightUserdata(_) => None,
    }
}

/// A number operand as-is; strings stay non-numbers (5.4+ `tonumberns`).
pub(super) fn as_number(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        _ => None,
    }
}

/// Arithmetic (not bitwise) on two numbers, PUC `luaO_rawarith`: integer
/// results for integer operands except `/` and `^`. The error is the
/// message of a zero integer divisor.
pub(crate) fn arith_num(
    version: LuaVersion,
    op: ArithOp,
    ln: Num,
    rn: Num,
) -> Result<Value, &'static str> {
    use ArithOp::*;
    Ok(match (op, ln, rn) {
        (Add, Num::Int(a), Num::Int(b)) => Value::Int(a.wrapping_add(b)),
        (Sub, Num::Int(a), Num::Int(b)) => Value::Int(a.wrapping_sub(b)),
        (Mul, Num::Int(a), Num::Int(b)) => Value::Int(a.wrapping_mul(b)),
        (IDiv, Num::Int(a), Num::Int(b)) => {
            if b == 0 {
                return Err("attempt to divide by zero");
            }
            Value::Int(int_idiv(a, b))
        }
        (Mod, Num::Int(a), Num::Int(b)) => {
            if b == 0 {
                return Err("attempt to perform 'n%0'");
            }
            Value::Int(int_mod(a, b))
        }
        (Add, a, b) => Value::Float(a.as_f64() + b.as_f64()),
        (Sub, a, b) => Value::Float(a.as_f64() - b.as_f64()),
        (Mul, a, b) => Value::Float(a.as_f64() * b.as_f64()),
        (Div, a, b) => Value::Float(a.as_f64() / b.as_f64()),
        (Pow, a, b) => Value::Float(num_pow(
            version >= LuaVersion::Lua54,
            a.as_f64(),
            b.as_f64(),
        )),
        (IDiv, a, b) => Value::Float((a.as_f64() / b.as_f64()).floor()),
        (Mod, a, b) => Value::Float(float_mod(version, a.as_f64(), b.as_f64())),
        (BAnd | BOr | BXor | Shl | Shr, ..) => unreachable!("bitwise op in arith_num"),
    })
}

/// `a ^ b`. 5.4+ `luai_numpow` squares by multiplying, which can differ
/// from the C library's `pow` in the last bit.
#[inline(always)]
pub(super) fn num_pow(v54: bool, a: f64, b: f64) -> f64 {
    if b == 2.0 && v54 { a * a } else { a.powf(b) }
}

/// Floor division of integers, `b != 0` (PUC `luaV_idiv`; `MIN // -1`
/// wraps to `MIN`).
#[inline(always)]
pub(super) fn int_idiv(a: i64, b: i64) -> i64 {
    let q = a.wrapping_div(b);
    if (a ^ b) < 0 && q.wrapping_mul(b) != a {
        q - 1
    } else {
        q
    }
}

/// Floor modulo of integers, `b != 0` (PUC `luaV_mod`; `MIN % -1` is 0).
#[inline(always)]
pub(super) fn int_mod(a: i64, b: i64) -> i64 {
    let m = a.wrapping_rem(b);
    if m != 0 && (m ^ b) < 0 { m + b } else { m }
}

/// A number's integer value, if it has one.
pub(super) fn int_of(n: Num) -> Option<i64> {
    match n {
        Num::Int(i) => Some(i),
        Num::Float(f) => crate::runtime::value::f2i_exact(f),
    }
}

/// A number, or a numeric string read as a float (5.2).
pub(super) fn coerce_num_float(v: Value) -> Option<Num> {
    match v {
        Value::Str(s) => numeric::str2num(s.as_bytes(), false, true),
        v => as_number(v),
    }
}

/// Number, or string coerced to number (5.3 string-arith coercion).
pub(super) fn coerce_num(v: Value) -> Option<Num> {
    match v {
        Value::Str(s) => numeric::str2num(s.as_bytes(), true, true),
        v => as_number(v),
    }
}

/// A number, or a numeric string read by C `strtod` (5.1 `luaO_str2d`).
pub(super) fn coerce_num_51(v: Value) -> Option<Num> {
    match v {
        Value::Str(s) => numeric::strtod_str(s.as_bytes()).map(Num::Float),
        v => as_number(v),
    }
}

/// A string's numeric value as the dialect converts it: 5.1 runs C
/// `strtod` over it (so `inf`/`nan` count and a NUL ends it), 5.2 has its
/// own reader but still only floats, 5.3+ `luaO_str2num` with integers.
pub(crate) fn str_to_num(s: &[u8], version: LuaVersion) -> Option<Num> {
    match version {
        LuaVersion::Lua51 => numeric::strtod_str(s).map(Num::Float),
        LuaVersion::Lua52 => numeric::str2num(s, false, true),
        _ => numeric::str2num(s, true, true),
    }
}

/// Lua shifts: logical on 64 bits; |shift| ≥ 64 yields 0; negative shifts
/// reverse direction.
pub(super) fn shift_left(a: i64, b: i64) -> i64 {
    if b < 0 {
        if b <= -64 {
            0
        } else {
            ((a as u64) >> (-b as u32)) as i64
        }
    } else if b >= 64 {
        0
    } else {
        ((a as u64) << (b as u32)) as i64
    }
}

/// i < f, exactly (PUC LTintfloat shape).
pub(super) fn int_lt_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    let ff = f.floor();
    let fi = ff as i64;
    if f == ff { i < fi } else { i <= fi }
}

/// i <= f, exactly.
pub(super) fn int_le_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    i <= f.floor() as i64
}

/// Clip a numeric `for` limit to the integer range (PUC forlimit). Returns
/// (clipped limit, loop-is-empty).
pub(super) fn int_for_limit(limit: Num, init: i64, step: i64) -> (i64, bool) {
    match limit {
        Num::Int(l) => {
            let empty = if step > 0 { init > l } else { init < l };
            (l, empty)
        }
        Num::Float(f) => {
            // PUC `forlimit` treats NaN like a limit below the integer
            // range (`0 < flim` is false): no run upward, down to minint
            // otherwise.
            if f.is_nan() {
                return if step > 0 {
                    (0, true)
                } else {
                    (i64::MIN, false)
                };
            }
            if step > 0 {
                if f >= 9_223_372_036_854_775_808.0 {
                    (i64::MAX, false)
                } else {
                    let l = f.floor();
                    if l < -9_223_372_036_854_775_808.0 {
                        (i64::MIN, true)
                    } else {
                        let li = l as i64;
                        (li, init > li)
                    }
                }
            } else if f <= -9_223_372_036_854_775_808.0 {
                (i64::MIN, false)
            } else {
                let l = f.ceil();
                if l >= 9_223_372_036_854_775_808.0 {
                    // PUC forlimit: a positive limit beyond the integer range
                    // is unreachable for a decreasing loop — empty.
                    (i64::MAX, true)
                } else {
                    let li = l as i64;
                    (li, init < li)
                }
            }
        }
    }
}
