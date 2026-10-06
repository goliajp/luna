//! Reading numbers: the dialect's `fscanf`-style scanners.

use super::*;

/// ≤5.2 read numbers with `fscanf("%lf")`. The BSD scanner converts the
/// longest prefix that is a valid floating-point numeral and pushes back
/// every byte after it; with no valid prefix, it pushes everything back.
pub(super) fn scan_double(u: Gc<Userdata>) -> std::io::Result<Value> {
    if u.crt.is_some() {
        let d = crt::with(u, msvc::scan::scan_double);
        return Ok(d.map_or(Value::Nil, Value::Float));
    }
    let mut c = getc(u)?;
    while matches!(c, Some(b) if is_c_space(b)) {
        c = getc(u)?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut commit = 0; // length of the longest complete numeral in buf
    let mut state = Scan::Start;
    while let Some(b) = c {
        let next = scan_step(state, b, &buf);
        let Some((st, complete)) = next else { break };
        buf.push(b);
        if complete {
            commit = buf.len();
        }
        state = st;
        c = getc(u)?;
    }
    if let Some(b) = c {
        buf.push(b);
    }
    unget(u, &buf[commit..]);
    if commit == 0 {
        return Ok(Value::Nil);
    }
    Ok(Value::Float(parse_c_double(&buf[..commit])))
}

fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

#[derive(Clone, Copy, PartialEq)]
enum Scan {
    Start,
    Sign,
    Zero,
    Int,
    Dot,
    Frac,
    ExpMark,
    ExpSign,
    ExpDigits,
    HexX,
    HexInt,
    HexDot,
    HexFrac,
    Word,
    NanOpen,
    Done,
}

/// One step of the `%lf` scanner: the state after `b`, and whether the
/// bytes so far form a complete numeral. `None` when `b` cannot continue.
fn scan_step(s: Scan, b: u8, buf: &[u8]) -> Option<(Scan, bool)> {
    let digits = |st| Some((st, true));
    match s {
        Scan::Start | Scan::Sign => match b {
            b'+' | b'-' if s == Scan::Start => Some((Scan::Sign, false)),
            b'0' => digits(Scan::Zero),
            b'1'..=b'9' => digits(Scan::Int),
            b'.' => Some((Scan::Dot, false)),
            b'i' | b'I' | b'n' | b'N' => Some((Scan::Word, false)),
            _ => None,
        },
        Scan::Zero if matches!(b, b'x' | b'X') => Some((Scan::HexX, false)),
        Scan::Zero | Scan::Int => match b {
            b'0'..=b'9' => digits(Scan::Int),
            b'.' => digits(Scan::Frac),
            b'e' | b'E' => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::Dot => match b {
            b'0'..=b'9' => digits(Scan::Frac),
            _ => None,
        },
        Scan::Frac => match b {
            b'0'..=b'9' => digits(Scan::Frac),
            b'e' | b'E' => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::ExpMark => match b {
            b'+' | b'-' => Some((Scan::ExpSign, false)),
            b'0'..=b'9' => digits(Scan::ExpDigits),
            _ => None,
        },
        Scan::ExpSign | Scan::ExpDigits => match b {
            b'0'..=b'9' => digits(Scan::ExpDigits),
            _ => None,
        },
        Scan::HexX => match b {
            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => digits(Scan::HexInt),
            b'.' => Some((Scan::HexDot, false)),
            _ => None,
        },
        Scan::HexInt => match b {
            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => digits(Scan::HexInt),
            b'.' => digits(Scan::HexFrac),
            b'p' | b'P' => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::HexDot | Scan::HexFrac => match b {
            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => digits(Scan::HexFrac),
            b'p' | b'P' if s == Scan::HexFrac => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::Word => {
            // "inf", "infinity", "nan", compared case-insensitively
            let word: Vec<u8> = buf
                .iter()
                .skip_while(|&&c| c == b'+' || c == b'-')
                .map(u8::to_ascii_lowercase)
                .chain(std::iter::once(b.to_ascii_lowercase()))
                .collect();
            if b"infinity".starts_with(&word) {
                Some((Scan::Word, word == b"inf" || word == b"infinity"))
            } else if b"nan".starts_with(&word) {
                Some((Scan::Word, word == b"nan"))
            } else if word == b"nan(" {
                Some((Scan::NanOpen, false))
            } else {
                None
            }
        }
        Scan::NanOpen => match b {
            b')' => Some((Scan::Done, true)),
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' => Some((Scan::NanOpen, false)),
            _ => None,
        },
        Scan::Done => None,
    }
}

/// `strtod` on a complete numeral from `scan_double`.
fn parse_c_double(s: &[u8]) -> f64 {
    let (neg, body) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    // glibc's scanf converts with strtod, and leaves its errno; the MSVC
    // library's (`msvc::scan`) leaves none
    let saved = crate::cerrno::get();
    let mag = if lower.starts_with(b"inf") {
        f64::INFINITY
    } else if lower.starts_with(b"nan") {
        f64::NAN
    } else {
        numeric::strtod_str(body).expect("the scanner only commits valid numerals")
    };
    if crate::cerrno::Lib::HOST != crate::cerrno::Lib::Glibc {
        crate::cerrno::set(saved);
    }
    if neg { -mag } else { mag }
}

/// Cap on a numeral's length for 5.3+'s reader (`L_MAXLENNUM`).
const L_MAXLENNUM: usize = 200;

/// 5.3+ `read_number`'s state: the look-ahead byte and the saved prefix.
struct Rn {
    buf: Vec<u8>,
    c: Option<u8>,
}

/// `nextc`: keep the look-ahead byte and read the next. Past
/// `L_MAXLENNUM` the numeral is invalidated and reading stops.
fn rn_next(rn: &mut Rn, u: Gc<Userdata>) -> std::io::Result<bool> {
    if rn.buf.len() >= L_MAXLENNUM {
        rn.buf.clear();
        return Ok(false);
    }
    if let Some(b) = rn.c {
        rn.buf.push(b);
    }
    rn.c = getc(u)?;
    Ok(true)
}

/// `test2`: take the look-ahead byte if it is one of `set`.
fn rn_test(rn: &mut Rn, u: Gc<Userdata>, set: &[u8]) -> std::io::Result<bool> {
    if matches!(rn.c, Some(c) if set.contains(&c)) {
        return rn_next(rn, u);
    }
    Ok(false)
}

/// `readdigits`.
fn rn_digits(rn: &mut Rn, u: Gc<Userdata>, hex: bool) -> std::io::Result<u32> {
    let mut count = 0;
    while matches!(rn.c, Some(c) if if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() })
        && rn_next(rn, u)?
    {
        count += 1;
    }
    Ok(count)
}

/// 5.3+ `read_number`'s scan: the longest prefix following a fixed numeral
/// grammar, with the first byte that does not fit pushed back.
pub(super) fn read_numeral(u: Gc<Userdata>) -> std::io::Result<Vec<u8>> {
    let mut c = getc(u)?;
    while matches!(c, Some(b) if is_c_space(b)) {
        c = getc(u)?;
    }
    let mut rn = Rn { buf: Vec::new(), c };
    let mut count = 0;
    let mut hex = false;
    rn_test(&mut rn, u, b"-+")?;
    if rn_test(&mut rn, u, b"0")? {
        if rn_test(&mut rn, u, b"xX")? {
            hex = true;
        } else {
            count = 1;
        }
    }
    count += rn_digits(&mut rn, u, hex)?;
    if rn_test(&mut rn, u, b".")? {
        count += rn_digits(&mut rn, u, hex)?;
    }
    if count > 0 && rn_test(&mut rn, u, if hex { b"pP" } else { b"eE" })? {
        rn_test(&mut rn, u, b"-+")?;
        rn_digits(&mut rn, u, false)?;
    }
    if let Some(b) = rn.c {
        unget(u, &[b]);
    }
    Ok(rn.buf)
}
