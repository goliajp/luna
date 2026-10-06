//! C `frexp` / `ldexp` on doubles, as PUC's math library calls them.

/// frexp: x = m * 2^e with 0.5 <= |m| < 1 (or m == x for 0/inf/nan).
pub(super) fn frexp(x: f64) -> (f64, i64) {
    if x == 0.0 || x.is_nan() || x.is_infinite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let exp_field = ((bits >> 52) & 0x7FF) as i64;
    if exp_field == 0 {
        // subnormal: normalize by scaling up, then adjust the exponent back
        let (m, e) = frexp(x * f64::from_bits(0x435u64 << 52)); // x * 2^54
        return (m, e - 54);
    }
    // force the stored exponent to represent 2^-1 so the mantissa lands in
    // [0.5, 1); the true exponent is then exp_field - 1022
    let m_bits = (bits & !(0x7FFu64 << 52)) | (1022u64 << 52);
    (f64::from_bits(m_bits), exp_field - 1022)
}

/// ldexp: m * 2^e, scaling in chunks so a large |e| can't overflow a single
/// power-of-two multiply.
pub(super) fn ldexp(mut m: f64, mut e: i64) -> f64 {
    if m == 0.0 || m.is_nan() || m.is_infinite() {
        return m;
    }
    while e > 1023 {
        m *= f64::from_bits(0x7FEu64 << 52); // 2^1023
        e -= 1023;
        if m == 0.0 || m.is_infinite() {
            return m;
        }
    }
    while e < -1022 {
        m *= f64::from_bits(0x001u64 << 52); // 2^-1022
        e += 1022;
        if m == 0.0 || m.is_infinite() {
            return m;
        }
    }
    m * f64::from_bits(((e + 1023) as u64) << 52)
}
