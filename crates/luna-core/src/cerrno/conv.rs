//! What converting a string to a number leaves in `errno`: the C calls each
//! dialect makes for it, `strtod` and Lua's own hexadecimal reader (which
//! ends in `ldexp`), on the part of the string they read. A string a
//! dialect rejects can still have been converted first: `"1e999z"` leaves
//! ERANGE in every dialect.

use super::{HexConv, Lib, after_strtod_decimal, apply, ldexp_errno, strtod_errno};

/// Which conversion runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    /// 5.1 `luaO_str2d`: `strtod`
    Lua51,
    /// 5.2 `luaO_str2d`: no `n` or `N`; with an `x` or `X`, Lua's reader,
    /// otherwise `strtod`
    Lua52,
    /// 5.3 and later `luaO_str2num`: an integer first; then, as the first
    /// of `.xXnN` in the string says, nothing (`n`), the platform's
    /// hexadecimal conversion (`x`) or `strtod`
    Later,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

fn hex(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

/// Apply what PUC's conversion of `s` (a C string: up to its first NUL)
/// leaves.
pub fn number(s: &[u8], d: Dialect) {
    let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
    match d {
        Dialect::Lua51 => {
            // `luaO_str2d` retries with `strtoul(s, &end, 16)` when `strtod`
            // stopped at an `x`; that never completes the string, but an
            // overflow leaves ERANGE
            if let Some(end) = strtod(s)
                && matches!(s.get(end), Some(b'x' | b'X'))
            {
                strtoul16(s);
            }
        }
        Dialect::Lua52 => {
            if s.iter().any(|c| matches!(c, b'n' | b'N')) {
                return;
            }
            if s.iter().any(|c| matches!(c, b'x' | b'X')) {
                lua_hex(s, false);
            } else {
                let _ = strtod(s);
            }
        }
        Dialect::Later => {
            if is_integer(s) {
                return;
            }
            match s.iter().find(|c| b".xXnN".contains(c)) {
                Some(b'n' | b'N') => {}
                Some(b'x' | b'X') if HexConv::LATER == HexConv::Own => lua_hex(s, true),
                _ => {
                    let _ = strtod(s);
                }
            }
        }
    }
}

/// `l_str2int`: spaces, a sign, decimal digits that fit or any hex digits,
/// spaces.
fn is_integer(s: &[u8]) -> bool {
    let mut i = s.iter().take_while(|&&c| is_space(c)).count();
    if matches!(s.get(i), Some(b'-' | b'+')) {
        i += 1;
    }
    let neg = i > 0 && s[i - 1] == b'-';
    let start;
    if s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
        start = i;
        while s.get(i).copied().and_then(hex).is_some() {
            i += 1;
        }
    } else {
        start = i;
        let mut a: u64 = 0;
        while let Some(d) = s.get(i).filter(|c| c.is_ascii_digit()) {
            let d = u64::from(d - b'0');
            let (maxby10, maxlast) = (i64::MAX as u64 / 10, i64::MAX as u64 % 10);
            if a >= maxby10 && (a > maxby10 || d > maxlast + u64::from(neg)) {
                return false;
            }
            a = a * 10 + d;
            i += 1;
        }
    }
    let digits = i > start;
    while s.get(i).is_some_and(|&c| is_space(c)) {
        i += 1;
    }
    digits && i == s.len()
}

/// `strtod` on `s`: the longest numeral at its start, after spaces and a
/// sign: decimal, hexadecimal after `0x`, or `inf` / `nan`, which set
/// nothing. Gives where the numeral ends, `None` when there is none.
fn strtod(s: &[u8]) -> Option<usize> {
    let mut i = s.iter().take_while(|&&c| is_space(c)).count();
    if matches!(s.get(i), Some(b'-' | b'+')) {
        i += 1;
    }
    let start = i;
    let s = &s[i..];
    let rest = |n: usize| -> Option<usize> { Some(start + n) };
    let word = |w: &[u8]| s.len() >= w.len() && s[..w.len()].eq_ignore_ascii_case(w);
    if word(b"infinity") {
        return rest(8);
    }
    if word(b"inf") {
        return rest(3);
    }
    if word(b"nan") {
        return rest(3);
    }
    if s.len() > 2 && s[0] == b'0' && matches!(s[1], b'x' | b'X') {
        let h = &s[2..];
        let int = h.iter().take_while(|&&c| hex(c).is_some()).count();
        let mut frac: &[u8] = &[];
        let mut j = int;
        if h.get(j) == Some(&b'.') {
            let n = h[j + 1..].iter().take_while(|&&c| hex(c).is_some()).count();
            frac = &h[j + 1..j + 1 + n];
            j += 1 + n;
        }
        // with no digit after it, the `0x` is just the numeral "0"
        if int + frac.len() == 0 {
            return rest(1);
        }
        let pexp = exponent(h, j, b"pP");
        let (x, nonzero, exact) = crate::numeric::hex_float(&h[..int], frac, pexp.unwrap_or(0));
        apply(strtod_errno(Lib::HOST, x, nonzero, exact));
        if pexp.is_some() {
            j += 1;
            if matches!(h.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            j += h[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        }
        return rest(2 + j);
    }
    let int = s.iter().take_while(|c| c.is_ascii_digit()).count();
    let mut j = int;
    let mut frac = 0;
    if s.get(j) == Some(&b'.') {
        frac = s[j + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        j += 1 + frac;
    }
    if int + frac == 0 {
        return None;
    }
    if exponent(s, j, b"eE").is_some() {
        j += 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        j += s[j..].iter().take_while(|c| c.is_ascii_digit()).count();
    }
    let text = std::str::from_utf8(&s[..j]).expect("an ASCII numeral");
    if let Ok(x) = text.parse::<f64>() {
        after_strtod_decimal(text, x);
    }
    rest(j)
}

/// `strtoul(s, _, 16)`: spaces, a sign, an optional `0x` (only before a
/// hex digit), hex digits; a value past `ULONG_MAX` (32 bits on Windows)
/// leaves ERANGE.
fn strtoul16(s: &[u8]) {
    let mut i = s.iter().take_while(|&&c| is_space(c)).count();
    if matches!(s.get(i), Some(b'-' | b'+')) {
        i += 1;
    }
    if s.get(i) == Some(&b'0')
        && matches!(s.get(i + 1), Some(b'x' | b'X'))
        && s.get(i + 2).copied().and_then(hex).is_some()
    {
        i += 2;
    }
    let max: u64 = if cfg!(windows) {
        u32::MAX as u64
    } else {
        u64::MAX
    };
    let mut n: u64 = 0;
    while let Some(d) = s.get(i).copied().and_then(hex) {
        match n.checked_mul(16).and_then(|n| n.checked_add(u64::from(d))) {
            Some(v) if v <= max => n = v,
            _ => {
                apply(Some(super::ERANGE));
                return;
            }
        }
        i += 1;
    }
}

/// The exponent at `s[at..]` after one of `marks`, when digits follow.
fn exponent(s: &[u8], at: usize, marks: &[u8]) -> Option<i64> {
    if !s.get(at).is_some_and(|c| marks.contains(c)) {
        return None;
    }
    let mut j = at + 1;
    let neg = s.get(j) == Some(&b'-');
    if matches!(s.get(j), Some(b'+' | b'-')) {
        j += 1;
    }
    let n = s[j..].iter().take_while(|c| c.is_ascii_digit()).count();
    let digits = &s[j..j + n];
    if digits.is_empty() {
        return None;
    }
    let e = digits
        .iter()
        .fold(0i64, |e, &c| (e * 10 + i64::from(c - b'0')).min(1 << 40));
    Some(if neg { -e } else { e })
}

/// `lua_strx2number`, Lua's own reader: `0x` after spaces and a sign, hex
/// digits with a point, and a binary exponent, accumulated in a double
/// (from 5.3, `sig30`, the first 30 significant digits only) that
/// `ldexp` scales. Without `0x` or without digits it returns at once.
fn lua_hex(s: &[u8], sig30: bool) {
    let mut i = s.iter().take_while(|&&c| is_space(c)).count();
    if matches!(s.get(i), Some(b'-' | b'+')) {
        i += 1;
    }
    if !(s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X'))) {
        return;
    }
    i += 2;
    let (mut r, mut e) = (0.0f64, 0i64);
    let (mut sig, mut any, mut dot) = (0, false, false);
    loop {
        match s.get(i) {
            Some(b'.') if !dot => dot = true,
            Some(&c) if hex(c).is_some() => {
                any = true;
                let d = hex(c).expect("a hex digit");
                if sig30 {
                    if sig == 0 && d == 0 {
                        // not significant
                    } else {
                        sig += 1;
                        if sig <= 30 {
                            r = r * 16.0 + f64::from(d);
                        } else {
                            e += 1;
                        }
                    }
                } else {
                    r = r * 16.0 + f64::from(d);
                }
                if dot {
                    e -= 1;
                }
            }
            _ => break,
        }
        i += 1;
    }
    if !any {
        return;
    }
    e *= 4;
    if matches!(s.get(i), Some(b'p' | b'P')) {
        match exponent(s, i, b"pP") {
            Some(p) => e += p,
            // 5.3 gives up here; 5.2 still scales what it read
            None if sig30 => return,
            None => {}
        }
    }
    apply(ldexp_errno(Lib::HOST, r, scale(r, e)));
}

/// `r * 2^e`, `r` finite.
fn scale(r: f64, e: i64) -> f64 {
    let mut x = r;
    let mut e = e.clamp(-4000, 4000);
    while e > 0 {
        let k = e.min(1000);
        x *= f64::from_bits(((1023 + k) as u64) << 52);
        e -= k;
    }
    while e < 0 {
        let k = (-e).min(1000);
        x /= f64::from_bits(((1023 + k) as u64) << 52);
        e += k;
    }
    x
}
