//! How full PUC's scanner fills its buffer (`ls->buff`) while it reads a
//! token: the buffer starts at 32 bytes and grows when a save would
//! overflow it, so the load counts its growth where PUC's scanner makes it.
//! Only names, numerals, strings and long comments can pass 32 bytes.

use crate::version::LuaVersion;

/// What kind of token a scanned string is for: which of them PUC's
/// scanner anchors, and re-anchors when a function ends, differs by
/// dialect.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanKind {
    Name,
    Str,
    Reserved,
}

/// The text of a scanned string: an interned name or literal of the
/// chunk, or a reserved word (its number among them, from 0) and its
/// place in the source.
#[derive(Clone, Copy)]
pub(crate) enum ScanText {
    Sym(crate::frontend::names::Sym),
    Reserved(u8, crate::frontend::span::Span),
}

/// How many reserved words there are (see [`ScanText::Reserved`]).
pub(crate) const RESERVED: usize = 23;

impl ScanText {
    /// The bytes, in the chunk's names and its source.
    pub(crate) fn bytes<'a>(
        self,
        names: &'a crate::frontend::names::Names,
        src: &'a [u8],
    ) -> &'a [u8] {
        match self {
            ScanText::Sym(s) => names.bytes(s),
            ScanText::Reserved(_, s) => s.slice(src),
        }
    }
}

/// What the scanner makes, by token: the strings, and how full its buffer
/// gets (see [`peak`]).
#[derive(Default)]
pub(crate) struct Scanned {
    pub(crate) strings: Vec<(u32, ScanText, ScanKind)>,
    pub(crate) peaks: Vec<(u32, u32)>,
}

/// The most bytes the scanner holds in its buffer while it reads the
/// whitespace and comments `gap` and then the token `raw` (its source
/// text).
pub(crate) fn peak(version: LuaVersion, gap: &[u8], raw: &[u8]) -> usize {
    let tok = match raw.first() {
        Some(b'"' | b'\'') => short_string(version, raw),
        Some(b'[') if raw.len() > 1 => raw.len() - long_open_newline(raw),
        // the numeral, then a terminating zero
        Some(b'0'..=b'9') => raw.len() + 1,
        Some(b'.') if raw.get(1).is_some_and(u8::is_ascii_digit) => raw.len() + 1,
        _ => raw.len(),
    };
    tok.max(comments(version, gap))
}

/// The newline right after a long bracket's opening, which the scanner
/// skips (one or a `\r\n` / `\n\r` pair), counting the newlines of the
/// text as one byte each.
fn long_open_newline(raw: &[u8]) -> usize {
    let open = raw.iter().skip(1).take_while(|&&c| c == b'=').count() + 2;
    let body = raw.get(open..).unwrap_or(&[]);
    let mut drop = newline_len(body);
    // each `\r\n` / `\n\r` pair inside is saved as one `\n`
    let mut i = drop;
    while i < body.len() {
        let n = newline_len(&body[i..]);
        if n == 2 {
            drop += 1;
        }
        i += n.max(1);
    }
    drop
}

/// The length of the newline at the start of `s`: 0, 1 or 2 (a pair of
/// different newline bytes counts as one).
fn newline_len(s: &[u8]) -> usize {
    match s {
        [a @ (b'\n' | b'\r'), b, ..] if (*b == b'\n' || *b == b'\r') && a != b => 2,
        [b'\n' | b'\r', ..] => 1,
        _ => 0,
    }
}

/// A quoted string. 5.1 and 5.2 save only what the string holds; from 5.3
/// on an escape keeps its `\` and its digits in the buffer until it is
/// decoded.
fn short_string(version: LuaVersion, raw: &[u8]) -> usize {
    let keeps = version >= LuaVersion::Lua53;
    let body = raw.get(1..raw.len().saturating_sub(1)).unwrap_or(&[]);
    // the opening quote is saved
    let (mut len, mut peak) = (1usize, 1usize);
    let mut i = 0;
    while i < body.len() {
        if body[i] != b'\\' {
            len += 1;
            peak = peak.max(len);
            i += 1;
            continue;
        }
        i += 1;
        let c = body.get(i).copied().unwrap_or(0);
        // the bytes held while the escape is read, and the bytes it saves
        let (held, saved, used) = match c {
            b'x' => (3, 1, 3),
            b'z' => {
                let mut j = i + 1;
                while j < body.len() && body[j].is_ascii_whitespace() {
                    j += 1;
                }
                (1, 0, j - i)
            }
            b'u' => {
                let hex = body.get(i + 2..).unwrap_or(&[]);
                let digits = hex.iter().take_while(|c| c.is_ascii_hexdigit()).count();
                let s = std::str::from_utf8(&hex[..digits]).unwrap_or("0");
                let v = u32::from_str_radix(s, 16).unwrap_or(0);
                (3 + digits, utf8_len(v), digits + 3)
            }
            b'0'..=b'9' => {
                let digits = body[i.min(body.len())..]
                    .iter()
                    .take(3)
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                (1 + digits, 1, digits)
            }
            b'\n' | b'\r' => (1, 1, newline_len(&body[i..])),
            _ => (1, 1, 1),
        };
        if keeps {
            peak = peak.max(len + held);
        }
        len += saved;
        peak = peak.max(len);
        i += used;
    }
    // the closing quote
    peak.max(len + 1)
}

/// The bytes PUC's `luaO_utf8esc` makes of `x` (up to 6, for values up to
/// 2^31).
fn utf8_len(x: u32) -> usize {
    match x {
        0..=0x7f => 1,
        0x80..=0x7ff => 2,
        0x800..=0xffff => 3,
        0x1_0000..=0x1f_ffff => 4,
        0x20_0000..=0x3ff_ffff => 5,
        _ => 6,
    }
}

/// The comments in `gap`. A long comment's bracket is saved, but not its
/// text, except for each `]` with its `=`s, and each newline, after which
/// the buffer is emptied.
fn comments(version: LuaVersion, gap: &[u8]) -> usize {
    let mut peak = 0;
    let mut i = 0;
    while i < gap.len() {
        if !gap[i..].starts_with(b"--") {
            i += 1;
            continue;
        }
        i += 2;
        if gap.get(i) != Some(&b'[') {
            i = line_end(gap, i);
            continue;
        }
        let eqs = gap[i + 1..].iter().take_while(|&&c| c == b'=').count();
        // `skip_sep` saves the bracket and its `=`s
        peak = peak.max(1 + eqs);
        if gap.get(i + 1 + eqs) != Some(&b'[') {
            i = line_end(gap, i);
            continue;
        }
        i += eqs + 2;
        // the second `[` is saved after the buffer is emptied
        let mut len = 1usize;
        peak = peak.max(len);
        i += newline_len(&gap[i..]);
        while i < gap.len() {
            match gap[i] {
                b']' | b'[' if gap[i] == b']' || version == LuaVersion::Lua51 => {
                    let n = gap[i + 1..].iter().take_while(|&&c| c == b'=').count();
                    len += 1 + n;
                    peak = peak.max(len);
                    let closes = gap.get(i + 1 + n) == Some(&gap[i]) && n == eqs;
                    i += 1 + n;
                    if closes {
                        len += 1;
                        peak = peak.max(len);
                        i += 1;
                        if gap[i - 1] == b']' {
                            break;
                        }
                    }
                }
                b'\n' | b'\r' => {
                    peak = peak.max(len + 1);
                    len = 0;
                    i += newline_len(&gap[i..]);
                }
                _ => i += 1,
            }
        }
    }
    peak
}

/// Where the line holding `i` ends.
fn line_end(s: &[u8], i: usize) -> usize {
    s[i..]
        .iter()
        .position(|&c| c == b'\n' || c == b'\r')
        .map_or(s.len(), |p| i + p)
}

/// The scanner buffer's size after it holds `peak` bytes, from `size`:
/// doubled each time, 5.5 grows it by half.
pub(crate) fn grown(version: LuaVersion, mut size: usize, peak: usize) -> usize {
    while size < peak {
        size += if version >= LuaVersion::Lua55 {
            size >> 1
        } else {
            size
        };
    }
    size
}
