//! Lua numeral conversion core (stone candidate: pure functions, no runtime
//! types). Two consumers: the lexer (literal tokens, shape pre-validated by
//! scanning) and the VM/stdlib (`str2num` — luaO_str2num semantics with
//! whitespace and sign). Versioning is expressed as capability flags so this
//! module stays dialect-agnostic.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    fn negate(self) -> Num {
        match self {
            Num::Int(i) => Num::Int(i.wrapping_neg()),
            Num::Float(f) => Num::Float(-f),
        }
    }
}

pub fn hex_digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// Decimal numeral (no sign, no surrounding space).
/// `int_ok = false` forces float results (Lua 5.1: numbers are doubles).
/// `neg` is whether a leading '-' was stripped by the caller: it widens the
/// integer range by one unit (PUC l_str2int's `+ neg`) so that the magnitude
/// 2^63 parses as an integer — letting `tonumber("-9223372036854775808")`
/// recover minint. The caller still applies the actual negation.
pub fn dec_literal(text: &[u8], int_ok: bool, neg: bool) -> Option<Num> {
    let mut i = 0;
    let mut int_digits = 0;
    while i < text.len() && text[i].is_ascii_digit() {
        i += 1;
        int_digits += 1;
    }
    let mut frac_digits = 0;
    let mut has_dot = false;
    if i < text.len() && text[i] == b'.' {
        has_dot = true;
        i += 1;
        while i < text.len() && text[i].is_ascii_digit() {
            i += 1;
            frac_digits += 1;
        }
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    let mut has_exp = false;
    if i < text.len() && matches!(text[i], b'e' | b'E') {
        has_exp = true;
        i += 1;
        if i < text.len() && matches!(text[i], b'+' | b'-') {
            i += 1;
        }
        let mut digits = 0;
        while i < text.len() && text[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
        if digits == 0 {
            return None;
        }
    }
    if i != text.len() {
        return None;
    }
    let s = str::from_utf8(text).expect("ascii numeral");
    if !has_dot && !has_exp && int_ok {
        // decimal integer; accumulate the magnitude in u64 with PUC's overflow
        // rule (l_str2int). The `+ neg` widens the last accepted digit so the
        // magnitude 2^63 is taken as an integer when negative (== minint);
        // on overflow it becomes a float. The caller applies the sign, so we
        // return the wrapped magnitude (2^63 as i64 is the minint bit pattern).
        const MAXBY10: u64 = i64::MAX as u64 / 10;
        const MAXLAST: u64 = i64::MAX as u64 % 10;
        let mut a: u64 = 0;
        let mut overflow = false;
        for &c in s.as_bytes() {
            let d = (c - b'0') as u64;
            if a >= MAXBY10 && (a > MAXBY10 || d > MAXLAST + neg as u64) {
                overflow = true;
                break;
            }
            a = a * 10 + d;
        }
        if !overflow {
            return Some(Num::Int(a as i64));
        }
    }
    s.parse::<f64>().ok().map(Num::Float)
}

/// Hex numeral after the `0x` prefix (no sign, no surrounding space).
pub fn hex_literal(text: &[u8], int_ok: bool, float_ok: bool) -> Option<Num> {
    let mut i = 0;
    while i < text.len() && hex_digit(text[i]).is_some() {
        i += 1;
    }
    let int_end = i;
    let mut has_dot = false;
    let mut frac = 0..0;
    if i < text.len() && text[i] == b'.' {
        has_dot = true;
        i += 1;
        let fs = i;
        while i < text.len() && hex_digit(text[i]).is_some() {
            i += 1;
        }
        frac = fs..i;
    }
    if int_end + frac.len() == 0 {
        return None;
    }
    let has_exp = i < text.len() && matches!(text[i], b'p' | b'P');
    let mut pexp: i64 = 0;
    if has_exp {
        i += 1;
        let mut sign = 1i64;
        if i < text.len() && matches!(text[i], b'+' | b'-') {
            sign = if text[i] == b'-' { -1 } else { 1 };
            i += 1;
        }
        let mut digits = 0;
        let mut e: i64 = 0;
        while i < text.len() && text[i].is_ascii_digit() {
            e = (e * 10 + (text[i] - b'0') as i64).min(1 << 40);
            i += 1;
            digits += 1;
        }
        if digits == 0 {
            return None;
        }
        pexp = sign * e;
    }
    if i != text.len() {
        return None;
    }
    if !has_exp && !has_dot {
        if int_ok {
            // pure hex integer: wraps modulo 2^64 (5.3+ semantics)
            let mut v: u64 = 0;
            for &c in &text[..int_end] {
                v = v
                    .wrapping_mul(16)
                    .wrapping_add(hex_digit(c).unwrap() as u64);
            }
            return Some(Num::Int(v as i64));
        }
        // ≤5.2 had no integer subtype: PUC `lua_strx2number` accumulates
        // every hex digit in `lua_Number` (a double), so a 150-digit literal
        // gives the actual mathematical value (~4e180) rather than the
        // wrapped low-64 bits. math.lua 5.2 :59 bakes that exact equality.
        let mut v: f64 = 0.0;
        for &c in &text[..int_end] {
            v = v * 16.0 + hex_digit(c).unwrap() as f64;
        }
        return Some(Num::Float(v));
    }
    if !float_ok {
        return None;
    }
    // value = mant * 2^(4*exp4 + pexp); digits beyond 64 mantissa bits fold
    // into the exponent (integer part) or the sticky bit (fraction part)
    let mut mant: u64 = 0;
    let mut sticky = false;
    let mut exp4: i64 = 0;
    for &c in &text[..int_end] {
        let d = hex_digit(c).unwrap() as u64;
        if mant >> 60 == 0 {
            mant = mant * 16 + d;
        } else {
            sticky |= d != 0;
            exp4 += 1;
        }
    }
    for &c in &text[frac] {
        let d = hex_digit(c).unwrap() as u64;
        if mant >> 60 == 0 {
            mant = mant * 16 + d;
            exp4 -= 1;
        } else {
            sticky |= d != 0;
        }
    }
    Some(Num::Float(compose_f64(mant, sticky, exp4 * 4 + pexp)))
}

/// luaO_str2num: optional surrounding whitespace and sign, decimal or hex.
/// Used by VM string→number coercion and `tonumber`.
pub fn str2num(s: &[u8], int_ok: bool, hex_float_ok: bool) -> Option<Num> {
    let is_space = |c: &&u8| matches!(**c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r');
    let mut s = s;
    while s.first().filter(is_space).is_some() {
        s = &s[1..];
    }
    while s.last().filter(is_space).is_some() {
        s = &s[..s.len() - 1];
    }
    let neg = match s.first() {
        Some(b'-') => {
            s = &s[1..];
            true
        }
        Some(b'+') => {
            s = &s[1..];
            false
        }
        _ => false,
    };
    let n = if s.len() > 2 && s[0] == b'0' && matches!(s[1], b'x' | b'X') {
        hex_literal(&s[2..], int_ok, hex_float_ok)?
    } else {
        dec_literal(s, int_ok, neg)?
    };
    Some(if neg { n.negate() } else { n })
}

/// Round a 64-bit mantissa (+sticky) to f64 and scale by 2^exp.
fn compose_f64(mant: u64, sticky: bool, exp: i64) -> f64 {
    if mant == 0 {
        return 0.0;
    }
    let bits = 64 - mant.leading_zeros() as i64;
    let (m, extra) = if bits <= 53 {
        (mant, 0i64)
    } else {
        let excess = (bits - 53) as u32;
        let kept = mant >> excess;
        let rem = mant & ((1u64 << excess) - 1);
        let half = 1u64 << (excess - 1);
        let round_up = rem > half || (rem == half && (sticky || kept & 1 == 1));
        (kept + round_up as u64, excess as i64)
    };
    scale_f64(m as f64, exp + extra)
}

fn exp2(e: i64) -> f64 {
    debug_assert!((-1022..=1023).contains(&e));
    f64::from_bits(((e + 1023) as u64) << 52)
}

fn scale_f64(mut f: f64, mut e: i64) -> f64 {
    while e > 1023 {
        f *= exp2(1023);
        e -= 1023;
        if f.is_infinite() {
            return f;
        }
    }
    while e < -1022 {
        f *= exp2(-1022);
        e += 1022;
        if f == 0.0 {
            return f;
        }
    }
    f * exp2(e)
}

/// Lua number → text. Integers print as integers. Floats print with
/// shortest round-trip digits (the 5.5 "read back correctly" rule) in
/// C `%g`-style presentation: scientific form when the decimal exponent
/// falls outside [-4, 14), two-digit signed exponent, and `.0` appended to
/// integral-looking decimals (PUC lua_number2str). Exact boundary alignment
/// against PUC 5.5 output is rechecked by the P04 gate (strings/math suites).
pub fn num_to_string(n: Num) -> String {
    num_to_string_for(n, /* legacy_float = */ false)
}

/// Write i64 decimal into a stack buffer; returns the slice of valid
/// bytes inside `buf`. 20 chars covers i64::MIN..=i64::MAX (the longest
/// is "-9223372036854775808" at 20 bytes). Hot in tostring(int) on
/// numeric-heavy workloads (string_concat builds 5000 of these): skips
/// the String allocation that `i.to_string()` does.
#[inline]
pub fn write_i64_dec(i: i64, buf: &mut [u8; 20]) -> &[u8] {
    if i == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let neg = i < 0;
    // unsigned_abs handles i64::MIN safely (negation overflow case).
    let mut n = i.unsigned_abs();
    let mut pos = 20;
    while n > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if neg {
        pos -= 1;
        buf[pos] = b'-';
    }
    &buf[pos..]
}

/// Variant for ≤5.2: those dialects only had `lua_Number` (a double), so
/// PUC's `%.14g` formatter trims any trailing `.0` (an integer-valued float
/// renders as plain `2`, not `2.0`). 5.3+ introduced the integer subtype and
/// the renderer started appending `.0` to distinguish floats — pm.lua's
/// pattern transformations build `"%" .. (s+1)` and need `"%2"` on 5.1/5.2.
pub fn num_to_string_for(n: Num, legacy_float: bool) -> String {
    match n {
        Num::Int(i) => i.to_string(),
        Num::Float(f) => float_to_string(f, legacy_float),
    }
}

fn float_to_string(f: f64, legacy_float: bool) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    // decimal exponent from Rust's shortest scientific form "d[.ddd]e±x"
    let sci = format!("{f:e}");
    let epos = sci.rfind('e').expect("scientific form has exponent");
    let exp: i32 = sci[epos + 1..].parse().expect("valid exponent");
    if (-4..14).contains(&exp) {
        let s = format!("{f}");
        if s.bytes().all(|c| c.is_ascii_digit() || c == b'-') {
            if legacy_float {
                s
            } else {
                format!("{s}.0")
            }
        } else {
            s
        }
    } else {
        let mantissa = &sci[..epos];
        let (esign, eabs) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
        format!("{mantissa}e{esign}{eabs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str2num_semantics() {
        assert_eq!(str2num(b"  42  ", true, true), Some(Num::Int(42)));
        assert_eq!(str2num(b"-10", true, true), Some(Num::Int(-10)));
        assert_eq!(str2num(b"+0x10", true, true), Some(Num::Int(16)));
        assert_eq!(str2num(b"-0x10", true, true), Some(Num::Int(-16)));
        assert_eq!(str2num(b" 0x1p4 ", true, true), Some(Num::Float(16.0)));
        assert_eq!(str2num(b"3.5", true, true), Some(Num::Float(3.5)));
        assert_eq!(str2num(b"1e3", true, true), Some(Num::Float(1000.0)));
        assert_eq!(str2num(b"", true, true), None);
        assert_eq!(str2num(b" - 1", true, true), None);
        assert_eq!(str2num(b"10a", true, true), None);
        assert_eq!(str2num(b"0x", true, true), None);
        // 5.1 flavor: everything is a float, no hex floats
        assert_eq!(str2num(b"42", false, false), Some(Num::Float(42.0)));
        assert_eq!(str2num(b"0x1p4", false, false), None);
        // minint boundary: "-9223372036854775808" parses as the integer minint
        // (PUC l_str2int's `+ neg`), but the positive magnitude 2^63 overflows
        // to a float, and maxint stays an integer.
        assert_eq!(
            str2num(b"-9223372036854775808", true, true),
            Some(Num::Int(i64::MIN))
        );
        assert_eq!(
            str2num(b"9223372036854775807", true, true),
            Some(Num::Int(i64::MAX))
        );
        assert_eq!(
            str2num(b"9223372036854775808", true, true),
            Some(Num::Float(9223372036854775808.0))
        );
    }

    #[test]
    fn number_printing() {
        assert_eq!(num_to_string(Num::Int(42)), "42");
        assert_eq!(num_to_string(Num::Int(-1)), "-1");
        assert_eq!(num_to_string(Num::Float(2.0)), "2.0");
        assert_eq!(num_to_string(Num::Float(-2.0)), "-2.0");
        assert_eq!(num_to_string(Num::Float(0.5)), "0.5");
        assert_eq!(num_to_string(Num::Float(1e300)), "1e+300");
        assert_eq!(num_to_string(Num::Float(1e-7)), "1e-07");
        assert_eq!(num_to_string(Num::Float(1e15)), "1e+15");
        assert_eq!(num_to_string(Num::Float(100.0)), "100.0");
        assert_eq!(num_to_string(Num::Float(f64::INFINITY)), "inf");
        assert_eq!(num_to_string(Num::Float(f64::NAN)), "nan");
        // shortest round-trip (the 5.5 printing rule)
        assert_eq!(num_to_string(Num::Float(0.1)), "0.1");
        assert_eq!(num_to_string(Num::Float(1.0 / 3.0)), "0.3333333333333333");
    }

    #[test]
    fn hex_float_rounding() {
        // > 53 significant bits forces rounding; Rust's u64→f64 conversion is
        // correctly rounded and serves as the reference
        let Some(Num::Float(f)) = hex_literal(b"1FFFFFFFFFFFFF8.0p0", true, true) else {
            panic!()
        };
        assert_eq!(f, 0x1FFFFFFFFFFFFF8u64 as f64);
        let Some(Num::Float(g)) = hex_literal(b"1.8p1", true, true) else {
            panic!()
        };
        assert_eq!(g, 3.0);
        let Some(Num::Float(h)) = hex_literal(b"0.8", true, true) else {
            panic!()
        };
        assert_eq!(h, 0.5);
    }
}
