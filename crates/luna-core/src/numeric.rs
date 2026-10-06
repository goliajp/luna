//! Lua numeral conversion core (stone candidate: pure functions, no runtime
//! types). Two consumers: the lexer (literal tokens, shape pre-validated by
//! scanning) and the VM/stdlib (`str2num` — luaO_str2num semantics with
//! whitespace and sign). Versioning is expressed as capability flags so this
//! module stays dialect-agnostic.

mod format;
pub use crate::cerrno::HexConv;
pub use format::*;

/// Result of parsing a Lua numeric literal — either an integer or a float
/// (Lua 5.1 collapses everything to float at this layer).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Num {
    /// Integer-typed numeral.
    Int(i64),
    /// Float-typed numeral.
    Float(f64),
}

impl Num {
    /// Lossy conversion to `f64`. Integer-to-float follows Lua's coercion
    /// (`Int` is cast as `i as f64`).
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

/// Decode a single ASCII hex digit (`0-9`, `a-f`, `A-F`) into its numeric
/// value, or return `None` for a non-hex byte.
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
    let x = s.parse::<f64>().ok()?;
    crate::cerrno::after_strtod_decimal(s, x);
    Some(Num::Float(x))
}

/// Hex numeral after the `0x` prefix (no sign, no surrounding space);
/// `conv` says which C conversion PUC runs, for the `errno` it leaves.
pub fn hex_literal(text: &[u8], int_ok: bool, float_ok: bool, conv: HexConv) -> Option<Num> {
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
    let x = compose_f64(mant, sticky, exp4 * 4 + pexp);
    let exact = !sticky && mant != 0 && {
        let tz = mant.trailing_zeros() as i64;
        let e = exp4 * 4 + pexp + tz;
        64 - (mant >> tz).leading_zeros() as i64 <= 53 && e >= -1074 && x.is_finite()
    };
    crate::cerrno::after_hex(conv, x, mant != 0 || sticky, exact);
    Some(Num::Float(x))
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
        // 5.2 (no integers) has its own reader; 5.3 and later, the platform's
        let conv = if int_ok { HexConv::LATER } else { HexConv::Own };
        hex_literal(&s[2..], int_ok, hex_float_ok, conv)?
    } else {
        dec_literal(s, int_ok, neg)?
    };
    Some(if neg { n.negate() } else { n })
}

/// C99 `strtod` applied to a C string, accepting the numeral only when
/// nothing but whitespace follows it: PUC 5.1's `luaO_str2d`. Unlike
/// [`str2num`] it reads `inf`/`infinity`/`nan` (any case, `nan(...)` too),
/// always yields a float, and stops at the first NUL, as C sees the string
/// end there. (5.1's retry with `strtoul` when `strtod` stops at an `x`
/// can never finish the string, so it is not modelled.)
pub fn strtod_str(s: &[u8]) -> Option<f64> {
    let is_space = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r');
    let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
    let mut i = 0;
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let rest = &s[i..];
    let starts = |w: &[u8]| rest.len() >= w.len() && rest[..w.len()].eq_ignore_ascii_case(w);
    let (v, used) = if starts(b"infinity") {
        (f64::INFINITY, 8)
    } else if starts(b"inf") {
        (f64::INFINITY, 3)
    } else if starts(b"nan") {
        let mut n = 3;
        // `nan(n-char-sequence)`
        if rest.get(3) == Some(&b'(') {
            let close = rest[4..]
                .iter()
                .position(|&c| !(c.is_ascii_alphanumeric() || c == b'_'));
            if let Some(k) = close
                && rest[4 + k] == b')'
            {
                n = 5 + k;
            }
        }
        (f64::NAN, n)
    } else if rest.len() > 2
        && rest[0] == b'0'
        && matches!(rest[1], b'x' | b'X')
        && let Some((v, n)) = hex_prefix(&rest[2..])
    {
        (v, 2 + n)
    } else {
        dec_prefix(rest)?
    };
    if !rest[used..].iter().all(|&c| is_space(c)) {
        return None;
    }
    Some(if neg { -v } else { v })
}

/// The longest decimal float numeral at the start of `s` (`strtod`'s
/// subject sequence) and its length.
fn dec_prefix(s: &[u8]) -> Option<(f64, usize)> {
    let digits = |s: &[u8], mut i: usize| {
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        i
    };
    let int_end = digits(s, 0);
    let mut i = int_end;
    let mut frac = 0;
    if i < s.len() && s[i] == b'.' {
        let e = digits(s, i + 1);
        frac = e - (i + 1);
        i = e;
    }
    if int_end + frac == 0 {
        return None;
    }
    if i < s.len() && matches!(s[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < s.len() && matches!(s[j], b'+' | b'-') {
            j += 1;
        }
        let e = digits(s, j);
        if e > j {
            i = e;
        }
    }
    let text = str::from_utf8(&s[..i]).expect("ascii numeral");
    let x = text.parse::<f64>().ok()?;
    crate::cerrno::after_strtod_decimal(text, x);
    Some((x, i))
}

/// The longest hex float numeral after `0x` at the start of `s`, and its
/// length; `None` when no hex digit follows (strtod then reads just "0").
fn hex_prefix(s: &[u8]) -> Option<(f64, usize)> {
    let hexes = |mut i: usize| {
        while i < s.len() && hex_digit(s[i]).is_some() {
            i += 1;
        }
        i
    };
    let int_end = hexes(0);
    let mut i = int_end;
    let mut frac = 0;
    if i < s.len() && s[i] == b'.' {
        let e = hexes(i + 1);
        frac = e - (i + 1);
        i = e;
    }
    if int_end + frac == 0 {
        return None;
    }
    if i < s.len() && matches!(s[i], b'p' | b'P') {
        let mut j = i + 1;
        if j < s.len() && matches!(s[j], b'+' | b'-') {
            j += 1;
        }
        let mut e = j;
        while e < s.len() && s[e].is_ascii_digit() {
            e += 1;
        }
        if e > j {
            i = e;
        }
    }
    hex_literal(&s[..i], false, true, HexConv::Strtod).map(|n| (n.as_f64(), i))
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

/// `x * y + z` as C code computes it where PUC is built: compilers for
/// aarch64 (gcc on Linux, clang on macOS) contract it into one fused
/// multiply-add, with a single rounding; elsewhere it rounds twice.
pub(crate) fn c_mul_add(x: f64, y: f64, z: f64) -> f64 {
    if cfg!(all(target_arch = "aarch64", not(target_os = "windows"))) {
        x.mul_add(y, z)
    } else {
        x * y + z
    }
}

/// 5.1/5.2 `luai_nummod`, `a - floor(a/b)*b`, contracted like
/// [`c_mul_add`]. The fused instruction takes a NaN from the addend `a`
/// first, then from the negated quotient, so a NaN can come out with the
/// opposite sign to the two-step result. That order is spelled out here
/// rather than left to which multiplicand the backend puts first.
pub(crate) fn nummod_floor(a: f64, b: f64) -> f64 {
    let q = (a / b).floor();
    if !cfg!(all(target_arch = "aarch64", not(target_os = "windows"))) {
        return a - q * b;
    }
    if a.is_nan() {
        // quieted, as the instruction returns it
        return a + 0.0;
    }
    if q.is_nan() {
        return -q;
    }
    (-q).mul_add(b, a)
}

/// C `fmod` as PUC's own build computes it on this platform, as the
/// interpreter's float `%` and `math.fmod` use it. Compiled traces reach
/// it through luna-jit's `luna_jit_fmod`.
#[doc(hidden)]
pub fn c_fmod(a: f64, b: f64) -> f64 {
    crate::vm::exec::c_fmod(a, b)
}

#[cfg(test)]
mod tests;
