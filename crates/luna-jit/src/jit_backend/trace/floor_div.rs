//! Lua's floor division and modulo on integers.

use super::*;

/// [`emit_floor_divmod`] for a divisor `k` known to be neither 0 nor -1:
/// the truncated remainder needs adjusting exactly when it is nonzero and
/// of the other sign than `k`, which for a known sign is one sign test,
/// done without a compare as an all-ones mask (`|r| < |k|`, so negating
/// `r` cannot overflow).
///
/// For `k > 0` there is a shorter exact form: with `s = x >> 63` (0 or
/// all ones), `t = x ^ s` is `x` or `-x - 1`, never negative and never
/// overflowing, and `floor(x / k) = (t / k) ^ s` with an unsigned
/// division, done as a multiply (see [`div_magic`]); the remainder is then
/// `x - k * q` with no sign adjustment.
pub(super) fn emit_floor_divmod_by<E: Emit>(bcx: &mut E, op: Op, a: Value, k: i64) -> Value {
    let kv = bcx.ins().iconst(types::I64, k);
    if k > 0 {
        let s = bcx.ins().sshr_imm_u(a, 63);
        let t = bcx.ins().bxor(a, s);
        let ut = match div_magic(k as u64) {
            None => t,
            Some((m, shift)) => {
                let mv = bcx.ins().iconst(types::I64, m as i64);
                let hi = bcx.ins().umulhi(t, mv);
                bcx.ins().ushr_imm_u(hi, i64::from(shift))
            }
        };
        let q = bcx.ins().bxor(ut, s);
        return if op == Op::IDiv {
            q
        } else {
            let qk = bcx.ins().imul(q, kv);
            bcx.ins().isub(a, qk)
        };
    }
    let q = bcx.ins().sdiv(a, kv);
    let qk = bcx.ins().imul(q, kv);
    let r = bcx.ins().isub(a, qk);
    let wrong_sign = if k > 0 { r } else { bcx.ins().ineg(r) };
    let mask = bcx.ins().sshr_imm_u(wrong_sign, 63);
    if op == Op::IDiv {
        bcx.ins().iadd(q, mask)
    } else {
        let adj = bcx.ins().band_imm_s(mask, k);
        bcx.ins().iadd(r, adj)
    }
}

/// `(m, shift)` with `n / d == umulhi(n, m) >> shift` for every `n` below
/// 2^63, or `None` for `d == 1`. The trace tiers compile without the
/// optimizer that would turn a division by a constant into this multiply,
/// and a 64-bit division takes tens of cycles.
///
/// With `l = ceil(log2 d)` and `m = ceil(2^(63 + l) / d)`, `m * d` exceeds
/// `2^(63 + l)` by less than `d <= 2^l`, so `floor(m * n / 2^(63 + l))` is
/// `floor(n / d)` for 63-bit `n` (Granlund and Montgomery, "Division by
/// invariant integers using multiplication", theorem 4.2), and `m < 2^64`
/// because `d > 2^(l - 1)`.
pub(super) fn div_magic(d: u64) -> Option<(u64, u32)> {
    if d <= 1 {
        return None;
    }
    let l = 64 - (d - 1).leading_zeros();
    let m = ((1u128 << (63 + l)) + u128::from(d) - 1) / u128::from(d);
    Some((m as u64, l - 1))
}

/// Lua's integer `//` or `%` for a nonzero divisor: rounded toward minus
/// infinity (the remainder takes the divisor's sign), and `x // -1` wraps
/// where a machine division by -1 would trap on minint.
pub(super) fn emit_floor_divmod<E: Emit>(bcx: &mut E, op: Op, a: Value, b: Value) -> Value {
    let minus_one = bcx.ins().iconst(types::I64, -1);
    let one = bcx.ins().iconst(types::I64, 1);
    let zero = bcx.ins().iconst(types::I64, 0);
    let is_m1 = bcx.ins().icmp(IntCC::Equal, b, minus_one);
    let safe_b = bcx.ins().select(is_m1, one, b);
    let q = bcx.ins().sdiv(a, safe_b);
    let qb = bcx.ins().imul(q, safe_b);
    let r = bcx.ins().isub(a, qb);
    // a nonzero remainder whose sign differs from the divisor's
    let r_nz = bcx.ins().icmp(IntCC::NotEqual, r, zero);
    let signs = bcx.ins().bxor(r, b);
    let differ = bcx.ins().icmp(IntCC::SignedLessThan, signs, zero);
    let adjust = bcx.ins().band(r_nz, differ);
    if op == Op::IDiv {
        let q1 = bcx.ins().isub(q, one);
        let floored = bcx.ins().select(adjust, q1, q);
        let neg = bcx.ins().ineg(a);
        bcx.ins().select(is_m1, neg, floored)
    } else {
        let rb = bcx.ins().iadd(r, b);
        let floored = bcx.ins().select(adjust, rb, r);
        bcx.ins().select(is_m1, zero, floored)
    }
}
