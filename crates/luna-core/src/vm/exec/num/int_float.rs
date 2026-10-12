//! Integers against floats: exact comparison and the numeric `for` limit.

use super::*;

/// i < f, exactly (PUC LTintfloat shape).
pub(in crate::vm::exec) fn int_lt_float(i: i64, f: f64) -> bool {
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
pub(in crate::vm::exec) fn int_le_float(i: i64, f: f64) -> bool {
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
pub(in crate::vm::exec) fn int_for_limit(limit: Num, init: i64, step: i64) -> (i64, bool) {
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
