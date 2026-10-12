//! `fscanf(f, "%lf", &d)` of the MSVC C library, which 5.1 and 5.2 read a
//! number with: what it takes from the stream and what it gives back
//! decide what the next read sees. A port of `parse_floating_point` over a
//! stream source, whose failed restores leave the stream where they are.

use super::{CrtFile, Os};

/// The character source: `getc` / `ungetc` with the count of bytes taken,
/// which a restore must find unchanged.
struct Source<'a> {
    f: &'a mut CrtFile,
    os: &'a mut dyn Os,
    count: i64,
    ok: bool,
}

impl Source<'_> {
    fn get(&mut self) -> u8 {
        self.count += 1;
        self.f.getc(self.os).unwrap_or(0)
    }

    fn unget(&mut self, c: u8) {
        self.count -= 1;
        if c != 0 {
            self.f.ungetc(c);
        }
    }

    fn restore(&mut self, saved: i64) -> bool {
        if saved != self.count {
            self.ok = false;
            return false;
        }
        true
    }
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn digit(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => u32::from(c - b'0'),
        b'a'..=b'z' => u32::from(c - b'a') + 10,
        b'A'..=b'Z' => u32::from(c - b'A') + 10,
        _ => u32::MAX,
    }
}

enum Parsed {
    NoDigits,
    Zero,
    Inf,
    QNan,
    SNan,
    Ind,
    Decimal(Vec<u8>, i64),
    Hex(Vec<u8>, i64),
}

/// Match `word` (upper or lower case per letter) starting at `c`.
fn next_chars(s: &mut Source<'_>, c: &mut u8, word: &[u8]) -> bool {
    for &w in word {
        if !c.eq_ignore_ascii_case(&w) {
            return false;
        }
        *c = s.get();
    }
    true
}

fn infinity(s: &mut Source<'_>, mut c: u8, saved: i64) -> Parsed {
    if !next_chars(s, &mut c, b"inf") {
        s.unget(c);
        s.restore(saved);
        return Parsed::NoDigits;
    }
    s.unget(c);
    let saved = s.count;
    c = s.get();
    if !next_chars(s, &mut c, b"inity") {
        s.unget(c);
        return if s.restore(saved) {
            Parsed::Inf
        } else {
            Parsed::NoDigits
        };
    }
    s.unget(c);
    Parsed::Inf
}

fn nan(s: &mut Source<'_>, mut c: u8, saved: i64) -> Parsed {
    if !next_chars(s, &mut c, b"nan") {
        s.unget(c);
        s.restore(saved);
        return Parsed::NoDigits;
    }
    s.unget(c);
    let saved = s.count;
    c = s.get();
    let fail = |s: &mut Source<'_>, c: u8| {
        s.unget(c);
        if s.restore(saved) {
            Parsed::QNan
        } else {
            Parsed::NoDigits
        }
    };
    if c != b'(' {
        return fail(s, c);
    }
    c = s.get();
    if next_chars(s, &mut c, b"snan)") {
        s.unget(c);
        return Parsed::SNan;
    }
    if next_chars(s, &mut c, b"ind)") {
        s.unget(c);
        return Parsed::Ind;
    }
    while c != b')' && c != 0 {
        if !(c.is_ascii_alphanumeric() || c == b'_') {
            return fail(s, c);
        }
        c = s.get();
    }
    if c != b')' {
        return fail(s, c);
    }
    Parsed::QNan
}

/// `parse_floating_point_from_source`, and whether the number is negative.
fn parse(s: &mut Source<'_>) -> (Parsed, bool) {
    let mut saved = s.count;
    let mut c = s.get();
    while is_space(c) {
        c = s.get();
    }
    let negative = c == b'-';
    if c == b'-' || c == b'+' {
        c = s.get();
    }
    if c == b'I' || c == b'i' {
        return (infinity(s, c, saved), negative);
    }
    if c == b'N' || c == b'n' {
        return (nan(s, c, saved), negative);
    }
    let mut hex = false;
    if c == b'0' {
        let next_saved = s.count;
        let next = s.get();
        if next == b'x' || next == b'X' {
            hex = true;
            c = s.get();
            saved = next_saved;
        } else {
            s.unget(next);
        }
    }
    let max = if hex { 15 } else { 9 };
    let mut digits = Vec::new();
    let mut adjust: i64 = 0;
    let mut found = false;
    while c == b'0' {
        found = true;
        c = s.get();
    }
    while digit(c) <= max {
        found = true;
        if digits.len() < 768 {
            digits.push(digit(c) as u8);
        }
        adjust += 1;
        c = s.get();
    }
    if c == b'.' {
        c = s.get();
        if digits.is_empty() {
            while c == b'0' {
                found = true;
                adjust -= 1;
                c = s.get();
            }
        }
        while digit(c) <= max {
            found = true;
            if digits.len() < 768 {
                digits.push(digit(c) as u8);
            }
            c = s.get();
        }
    }
    if !found {
        s.unget(c);
        if !s.restore(saved) {
            return (Parsed::NoDigits, negative);
        }
        return (if hex { Parsed::Zero } else { Parsed::NoDigits }, negative);
    }
    s.unget(c);
    saved = s.count;
    c = s.get();
    let has_exponent = match c {
        b'e' | b'E' => !hex,
        b'p' | b'P' => hex,
        _ => false,
    };
    let mut exponent: i64 = 0;
    if has_exponent {
        c = s.get();
        let exp_negative = c == b'-';
        if c == b'+' || c == b'-' {
            c = s.get();
        }
        let mut exp_digits = false;
        while c == b'0' {
            exp_digits = true;
            c = s.get();
        }
        while digit(c) < 10 {
            exp_digits = true;
            exponent = exponent * 10 + i64::from(digit(c));
            if exponent > 5200 {
                exponent = 5201;
                break;
            }
            c = s.get();
        }
        while digit(c) < 10 {
            c = s.get();
        }
        if exp_negative {
            exponent = -exponent;
        }
        if !exp_digits {
            s.unget(c);
            if s.restore(saved) {
                c = s.get();
            } else {
                return (Parsed::NoDigits, negative);
            }
        }
    }
    s.unget(c);
    while digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.is_empty() {
        return (Parsed::Zero, negative);
    }
    let exponent = exponent + adjust * if hex { 4 } else { 1 };
    (
        if hex {
            Parsed::Hex(digits, exponent)
        } else {
            Parsed::Decimal(digits, exponent)
        },
        negative,
    )
}

fn value(p: Parsed, negative: bool) -> Option<f64> {
    let sign = |x: f64| if negative { -x } else { x };
    Some(match p {
        Parsed::NoDigits => return None,
        Parsed::Zero => sign(0.0),
        Parsed::Inf => sign(f64::INFINITY),
        Parsed::QNan => sign(f64::from_bits(0x7ff8_0000_0000_0000)),
        Parsed::SNan => sign(f64::from_bits(0x7ff4_0000_0000_0000)),
        Parsed::Ind => f64::from_bits(0xfff8_0000_0000_0000),
        Parsed::Decimal(d, e) => {
            // 0.DIGITS × 10^e, correctly rounded
            let mut text = String::from("0.");
            text.extend(d.iter().map(|&x| char::from(b'0' + x)));
            text.push_str(&format!("e{e}"));
            sign(text.parse::<f64>().unwrap_or(0.0))
        }
        Parsed::Hex(d, e) => f64::from_bits(hex_bits(&d, e, negative)),
    })
}

/// 0x0.DIGITS × 2^e as the library computes it
/// (`convert_hexadecimal_string_to_floating_type` and
/// `assemble_floating_point_value`), including its slip on a value that
/// rounds up from the subnormal range to the smallest normal one (there the
/// exponent comes out 3 too high).
fn hex_bits(digits: &[u8], e: i64, negative: bool) -> u64 {
    const NORMAL_MASK: u64 = (1 << 53) - 1;
    const DENORMAL_MASK: u64 = (1 << 52) - 1;
    let mut mantissa: u64 = 0;
    let mut exponent = e + 52;
    let mut i = 0;
    while i < digits.len() && mantissa <= NORMAL_MASK {
        mantissa = mantissa * 16 + u64::from(digits[i]);
        exponent -= 4;
        i += 1;
    }
    let zero_tail = digits[i..].iter().all(|&d| d == 0);
    let sign = u64::from(negative) << 63;
    let bits = 64 - i64::from(mantissa.leading_zeros());
    let shift = 53 - bits;
    let normal = exponent - shift;
    let (mut m, mut exp) = (mantissa, normal);
    if normal > 1023 {
        return sign | 0x7ff0_0000_0000_0000;
    } else if normal < -1022 {
        let dshift = shift + normal + 1023 - 1;
        exp = -1023;
        if dshift < 0 {
            m = shift_rounding(m, -dshift, zero_tail);
            if m == 0 {
                return sign;
            }
            if m > DENORMAL_MASK {
                exp = exponent - (dshift + 1) - shift;
            }
        } else {
            m <<= dshift;
        }
    } else if shift < 0 {
        m = shift_rounding(m, -shift, zero_tail);
        if m > NORMAL_MASK {
            m >>= 1;
            exp += 1;
            if exp > 1023 {
                return sign | 0x7ff0_0000_0000_0000;
            }
        }
    } else if shift > 0 {
        m <<= shift;
    }
    sign | ((((exp + 1023) as u64) & 0x7ff) << 52) | (m & DENORMAL_MASK)
}

/// `right_shift_with_rounding`, to nearest with ties to even.
fn shift_rounding(value: u64, shift: i64, zero_tail: bool) -> u64 {
    if shift >= 64 {
        return 0;
    }
    let extra = (1u64 << (shift - 1)) - 1;
    let round = 1u64 << (shift - 1);
    let lsb = 1u64 << shift;
    let tail = !zero_tail || value & extra != 0;
    let up = value & round != 0 && (tail || value & lsb != 0);
    (value >> shift) + u64::from(up)
}

/// `fscanf(f, "%lf", &d) == 1`: the number, or `None` when the conversion
/// failed (or the stream was at its end).
pub(crate) fn scan_double(f: &mut CrtFile, os: &mut dyn Os) -> Option<f64> {
    let mut s = Source {
        f,
        os,
        count: 0,
        ok: true,
    };
    // the directive's own whitespace skip
    loop {
        let c = s.f.getc(s.os)?;
        if !is_space(c) {
            s.f.ungetc(c);
            break;
        }
    }
    let (p, negative) = parse(&mut s);
    if !s.ok || s.count == 0 {
        return None;
    }
    value(p, negative)
}
