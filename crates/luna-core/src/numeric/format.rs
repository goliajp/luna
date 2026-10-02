//! Number to text conversion.

use super::*;

/// Lua number → text. Integers print as integers. Floats print with
/// shortest round-trip digits (the 5.5 "read back correctly" rule) in
/// C `%g`-style presentation: scientific form when the decimal exponent
/// falls outside [-4, 14), two-digit signed exponent, and `.0` appended to
/// integral-looking decimals (PUC lua_number2str). Exact boundary alignment
/// against PUC 5.5 output is checked by the official strings/math suites.
pub fn num_to_string(n: Num) -> String {
    num_to_string_for(n, FloatFmt::TwoStage55)
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

/// Float rendering flavor per dialect generation — each PUC line
/// prints floats with a different `LUA_NUMBER_FMT` (v2.14 HD, pinned
/// by the per-dialect diff corpus):
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatFmt {
    /// ≤5.2: `%.14g`, and NO ".0" suffix (single number type —
    /// `tostring(2.0)` is `"2"`). pm.lua :13 and 5.1/511 pin this.
    Legacy14,
    /// 5.3/5.4: `%.14g` + ".0" on integer-looking renderings.
    G14,
    /// 5.5: `%.15g` → round-trip check → `%.17g` + ".0"
    /// (lobject.c `tostringbuffFloat`).
    TwoStage55,
}

/// Render a number per the dialect's float flavor. Integers are
/// flavor-independent.
pub fn num_to_string_for(n: Num, fmt: FloatFmt) -> String {
    match n {
        Num::Int(i) => i.to_string(),
        Num::Float(f) => float_to_string(f, fmt),
    }
}

/// C `printf("%.{prec}g", f)` semantics: `prec` significant digits;
/// scientific form when the decimal exponent is `< -4` or `>= prec`,
/// fixed form otherwise; trailing zeros (and a bare trailing point)
/// stripped; scientific exponent printed sign + ≥2 digits.
fn format_g(f: f64, prec: usize) -> String {
    debug_assert!(prec >= 1);
    // Decimal exponent from a correctly-rounded scientific rendering at
    // the target precision (rounding may bump the exponent: 9.99 → 1e1).
    let sci = format!("{f:.*e}", prec - 1);
    let epos = sci.rfind('e').expect("scientific form has exponent");
    let exp: i32 = sci[epos + 1..].parse().expect("valid exponent");
    if exp < -4 || exp >= prec as i32 {
        let mut mant = sci[..epos].to_string();
        if mant.contains('.') {
            while mant.ends_with('0') {
                mant.pop();
            }
            if mant.ends_with('.') {
                mant.pop();
            }
        }
        let (esign, eabs) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
        format!("{mant}e{esign}{eabs:02}")
    } else {
        let decimals = (prec as i32 - 1 - exp).max(0) as usize;
        let mut s = format!("{f:.decimals$}");
        if s.contains('.') {
            while s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.pop();
            }
        }
        s
    }
}

/// How the host C library's `printf` spells a NaN, as (shows a minus
/// sign, lowercase body). PUC renders numbers through the platform's
/// `printf`, which differs here, so luna follows the platform it runs on:
/// Apple's libc never prints a NaN's sign; glibc and musl print it like any
/// other sign (`-nan`, and `+nan` / ` nan` under those flags); the Windows
/// CRT also names the kind (`-nan(ind)` for the default NaN that 0/0 and
/// friends produce, `nan(snan)` for a signalling one). wasm follows musl,
/// whose `printf` wasi-libc uses.
pub(crate) fn nan_spelling(f: f64) -> (bool, &'static str) {
    let negative = f.is_sign_negative();
    if cfg!(target_vendor = "apple") {
        (false, "nan")
    } else if cfg!(target_os = "windows") {
        let bits = f.to_bits();
        let quiet = bits & (1 << 51) != 0;
        let body = if !quiet {
            "nan(snan)"
        } else if bits == 0xFFF8_0000_0000_0000 {
            "nan(ind)"
        } else {
            "nan"
        };
        (negative, body)
    } else {
        (negative, "nan")
    }
}

fn float_to_string(f: f64, fmt: FloatFmt) -> String {
    if f.is_nan() {
        let (negative, body) = nan_spelling(f);
        return if negative {
            format!("-{body}")
        } else {
            body.to_string()
        };
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let mut s = match fmt {
        // ≤5.4: plain LUA_NUMBER_FMT="%.14g" (lua 5.1.5-5.4.9).
        FloatFmt::Legacy14 | FloatFmt::G14 => format_g(f, 14),
        // 5.5 `tostringbuffFloat` (lobject.c): %.15g, read back, and
        // only if the round-trip is inexact re-print with %.17g.
        FloatFmt::TwoStage55 => {
            let first = format_g(f, 15);
            if first.parse::<f64>() == Ok(f) {
                first
            } else {
                format_g(f, 17)
            }
        }
    };
    if s.bytes().all(|c| c.is_ascii_digit() || c == b'-') && fmt != FloatFmt::Legacy14 {
        s.push_str(".0");
    }
    s
}
