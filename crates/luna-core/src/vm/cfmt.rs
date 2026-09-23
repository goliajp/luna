//! The C `printf` conversions `string.format` hands its items to, rendered
//! the way the reference platform's libc (Apple's, FreeBSD-derived) does.
//!
//! Only well-formed specifications reach here — at most five flags, two
//! width digits and two precision digits; each dialect's `string.format`
//! validates first. Flags C leaves undefined for a conversion behave as
//! that libc makes them behave: `0` pads `%s`/`%c` with zeros, `+`/space
//! are ignored for unsigned conversions, and a NaN never shows a sign.
//! One libc bug is not reproduced: its `%.Na` rounds a half by the parity
//! of the digit after the rounding position; here it rounds half to even.

/// A parsed conversion specification (flags, width, precision).
#[derive(Clone, Copy, Default)]
pub(crate) struct Spec {
    pub minus: bool,
    pub plus: bool,
    pub space: bool,
    pub hash: bool,
    pub zero: bool,
    pub width: usize,
    pub prec: Option<usize>,
}

impl Spec {
    /// Parse the bytes between '%' and the conversion letter.
    pub(crate) fn parse(body: &[u8]) -> Spec {
        let mut sp = Spec::default();
        let mut i = 0;
        while let Some(&c) = body.get(i) {
            match c {
                b'-' => sp.minus = true,
                b'+' => sp.plus = true,
                b' ' => sp.space = true,
                b'#' => sp.hash = true,
                b'0' => sp.zero = true,
                _ => break,
            }
            i += 1;
        }
        while let Some(&c @ b'0'..=b'9') = body.get(i) {
            sp.width = sp.width * 10 + (c - b'0') as usize;
            i += 1;
        }
        if body.get(i) == Some(&b'.') {
            i += 1;
            let mut p = 0;
            while let Some(&c @ b'0'..=b'9') = body.get(i) {
                p = p * 10 + (c - b'0') as usize;
                i += 1;
            }
            sp.prec = Some(p);
        }
        sp
    }

    fn sign(&self, negative: bool) -> &'static [u8] {
        if negative {
            b"-"
        } else if self.plus {
            b"+"
        } else if self.space {
            b" "
        } else {
            b""
        }
    }
}

/// Lay out `prefix` (sign, `0x`) and `body` in the field width: spaces on
/// the left, spaces on the right with `-`, or zeros between the two.
fn pad(out: &mut Vec<u8>, sp: &Spec, zeros: bool, prefix: &[u8], body: &[u8]) {
    let len = prefix.len() + body.len();
    let fill = sp.width.saturating_sub(len);
    if sp.minus {
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(b' ', fill));
    } else if zeros {
        out.extend_from_slice(prefix);
        out.extend(std::iter::repeat_n(b'0', fill));
        out.extend_from_slice(body);
    } else {
        out.extend(std::iter::repeat_n(b' ', fill));
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
    }
}

/// `%d` / `%i`.
pub(crate) fn signed(out: &mut Vec<u8>, sp: &Spec, v: i64) {
    let mut buf = [0u8; DIGITS_CAP];
    let start = int_digits(&mut buf, v.unsigned_abs(), 10, false, sp.prec);
    pad(
        out,
        sp,
        sp.zero && sp.prec.is_none(),
        sp.sign(v < 0),
        &buf[start..],
    );
}

/// `%u`, `%o`, `%x`, `%X` on the bits of `v`.
pub(crate) fn unsigned(out: &mut Vec<u8>, sp: &Spec, conv: u8, v: u64) {
    let base = match conv {
        b'o' => 8,
        b'x' | b'X' => 16,
        _ => 10,
    };
    let mut buf = [0u8; DIGITS_CAP];
    let mut start = int_digits(&mut buf, v, base, conv == b'X', sp.prec);
    // '#' makes the first octal digit a zero
    if sp.hash && conv == b'o' && buf.get(start) != Some(&b'0') {
        start -= 1;
        buf[start] = b'0';
    }
    let prefix: &[u8] = match conv {
        b'x' if sp.hash && v != 0 => b"0x",
        b'X' if sp.hash && v != 0 => b"0X",
        _ => b"",
    };
    pad(out, sp, sp.zero && sp.prec.is_none(), prefix, &buf[start..]);
}

/// Room for a two-digit precision plus an octal '#' zero.
const DIGITS_CAP: usize = 101;

/// Write the digits of `v` right-aligned in `buf`, zero-extended to the
/// precision (precision zero turns a zero value into no digits at all);
/// returns where they start.
fn int_digits(
    buf: &mut [u8; DIGITS_CAP],
    mut v: u64,
    base: u64,
    upper: bool,
    prec: Option<usize>,
) -> usize {
    let set: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut start = DIGITS_CAP;
    if !(v == 0 && prec == Some(0)) {
        loop {
            start -= 1;
            buf[start] = set[(v % base) as usize];
            v /= base;
            if v == 0 {
                break;
            }
        }
    }
    while DIGITS_CAP - start < prec.unwrap_or(0) {
        start -= 1;
        buf[start] = b'0';
    }
    start
}

/// `%c`.
pub(crate) fn char(out: &mut Vec<u8>, sp: &Spec, c: u8) {
    pad(out, sp, sp.zero, b"", &[c]);
}

/// `%s` on a C string: stops at the first zero byte; precision truncates.
pub(crate) fn cstr(out: &mut Vec<u8>, sp: &Spec, s: &[u8]) {
    let s = &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())];
    let s = match sp.prec {
        Some(p) if p < s.len() => &s[..p],
        _ => s,
    };
    pad(out, sp, sp.zero, b"", s);
}

/// `%e %E %f %F %g %G %a %A`.
pub(crate) fn float(out: &mut Vec<u8>, sp: &Spec, conv: u8, x: f64) {
    let upper = conv.is_ascii_uppercase();
    if x.is_nan() {
        let body: &[u8] = if upper { b"NAN" } else { b"nan" };
        pad(out, sp, false, b"", body);
        return;
    }
    let sign = sp.sign(x.is_sign_negative());
    if x.is_infinite() {
        let body: &[u8] = if upper { b"INF" } else { b"inf" };
        pad(out, sp, false, sign, body);
        return;
    }
    let x = x.abs();
    let mut body = match conv.to_ascii_lowercase() {
        b'f' => fixed(x, sp.prec.unwrap_or(6), sp.hash),
        b'e' => exponent(x, sp.prec.unwrap_or(6), sp.hash, upper),
        b'g' => general(x, sp.prec, sp.hash, upper),
        _ => {
            let (prefix, body) = hex(x, sp.prec, sp.hash, upper);
            let mut p = sign.to_vec();
            p.extend_from_slice(prefix);
            pad(out, sp, sp.zero, &p, &body);
            return;
        }
    };
    if upper {
        body.make_ascii_uppercase();
    }
    pad(out, sp, sp.zero, sign, &body);
}

/// `%.{prec}f` of a non-negative finite `x`, correctly rounded.
fn fixed(x: f64, prec: usize, hash: bool) -> Vec<u8> {
    let mut s = format!("{x:.prec$}").into_bytes();
    if hash && prec == 0 {
        s.push(b'.');
    }
    s
}

/// `%.{prec}e`: mantissa, then at least two exponent digits.
fn exponent(x: f64, prec: usize, hash: bool, upper: bool) -> Vec<u8> {
    let s = format!("{x:.prec$e}");
    let (mant, exp) = s.split_once('e').expect("rust exponent form");
    let exp: i32 = exp.parse().expect("rust exponent digits");
    let mut b = mant.as_bytes().to_vec();
    if hash && prec == 0 {
        b.push(b'.');
    }
    b.push(if upper { b'E' } else { b'e' });
    b.extend_from_slice(format!("{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs()).as_bytes());
    b
}

/// `%g`: `%e` or `%f` by the decimal exponent, trailing zeros dropped
/// unless `#`.
fn general(x: f64, prec: Option<usize>, hash: bool, upper: bool) -> Vec<u8> {
    let p = match prec {
        None => 6,
        Some(0) => 1,
        Some(p) => p,
    };
    let e = format!("{x:.*e}", p - 1);
    let exp: i64 = e
        .split_once('e')
        .expect("rust exponent form")
        .1
        .parse()
        .expect("digits");
    let mut b = if exp < -4 || exp >= p as i64 {
        exponent(x, p - 1, hash, upper)
    } else {
        fixed(x, (p as i64 - 1 - exp) as usize, hash)
    };
    if !hash && let Some(dot) = b.iter().position(|&c| c == b'.') {
        let mant_end = b
            .iter()
            .position(|&c| c == b'e' || c == b'E')
            .unwrap_or(b.len());
        let mut keep = mant_end;
        while keep > dot + 1 && b[keep - 1] == b'0' {
            keep -= 1;
        }
        if keep == dot + 1 {
            keep = dot;
        }
        b.drain(keep..mant_end);
    }
    b
}

/// The 53-bit significand (leading bit at bit 52) and binary exponent of a
/// finite positive `x`; subnormals are normalised, as the reference libc
/// prints them (`0x1p-1074`, not `0x0.0000000000001p-1022`).
fn significand(x: f64) -> (u64, i64) {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i64;
    let mant = bits & ((1 << 52) - 1);
    if exp == 0 {
        let shift = i64::from(mant.leading_zeros()) - 11;
        (mant << shift, -1022 - shift)
    } else {
        (mant | (1 << 52), exp - 1023)
    }
}

/// `%a`: prefix ("0x") and the rest. A precision rounds the significand to
/// that many hex digits, half to even; a carry out of the leading digit
/// leaves it at 2 (`0x2.0p+0`) as libc does.
fn hex(x: f64, prec: Option<usize>, hash: bool, upper: bool) -> (&'static [u8], Vec<u8>) {
    let (m, e) = if x == 0.0 { (0, 0) } else { significand(x) };
    // fraction digits: as many as asked, or all significant ones
    let nfrac = prec.unwrap_or_else(|| {
        (0..=13)
            .find(|&n| m & ((1 << (52 - 4 * n)) - 1) == 0)
            .expect("n == 13 matches")
    });
    let kept = nfrac.min(13);
    let shift = 52 - 4 * kept as u32;
    let mut q = m >> shift;
    if shift > 0 {
        let rem = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        if rem > half || (rem == half && q & 1 == 1) {
            q += 1;
        }
    }
    let set: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut b = format!("{}", q >> (4 * kept)).into_bytes();
    if nfrac > 0 || hash {
        b.push(b'.');
    }
    for i in (0..kept).rev() {
        b.push(set[((q >> (4 * i)) & 0xF) as usize]);
    }
    b.extend(std::iter::repeat_n(b'0', nfrac - kept));
    b.push(if upper { b'P' } else { b'p' });
    b.extend_from_slice(format!("{}{}", if e < 0 { '-' } else { '+' }, e.abs()).as_bytes());
    (if upper { b"0X" } else { b"0x" }, b)
}

/// `%p`: the libc's pointer rendering.
pub(crate) fn pointer(out: &mut Vec<u8>, sp: &Spec, p: usize) {
    pad(out, sp, false, b"", format!("{p:#x}").as_bytes());
}
