//! Byte-driven lexer. The source is an arbitrary byte sequence (Lua sources
//! and string literals are not required to be UTF-8); only `\u{...}` escapes
//! produce UTF-8 output.

use crate::frontend::error::SyntaxError;
use crate::frontend::span::Span;
use crate::frontend::token::{Token, TokenInfo};
use crate::version::LuaVersion;

pub struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
    line: u32,
    version: LuaVersion,
}

impl<'s> Lexer<'s> {
    pub fn new(src: &'s [u8], version: LuaVersion) -> Lexer<'s> {
        let mut lex = Lexer {
            src,
            pos: 0,
            line: 1,
            version,
        };
        // UTF-8 BOM + shebang, as in lua.c's file loading.
        if lex.src.starts_with(&[0xEF, 0xBB, 0xBF]) {
            lex.pos = 3;
        }
        if lex.cur() == Some(b'#') {
            while !matches!(lex.cur(), None | Some(b'\n') | Some(b'\r')) {
                lex.pos += 1;
            }
        }
        lex
    }

    pub fn src(&self) -> &'s [u8] {
        self.src
    }

    fn cur(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    /// Consume `\n`, `\r`, `\n\r` or `\r\n` as a single line break.
    fn newline(&mut self) {
        let first = self.cur();
        self.bump();
        if let (Some(a), Some(b)) = (first, self.cur())
            && (b == b'\n' || b == b'\r')
            && b != a
        {
            self.bump();
        }
        self.line += 1;
    }

    fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into(),
        }
    }

    fn err_near(&self, line: u32, msg: &str, start: usize) -> SyntaxError {
        let text = String::from_utf8_lossy(&self.src[start..self.pos]);
        self.err(line, format!("{msg} near '{text}'"))
    }

    pub fn next_token(&mut self) -> Result<TokenInfo, SyntaxError> {
        loop {
            let start = self.pos;
            let line = self.line;
            let Some(c) = self.cur() else {
                return Ok(TokenInfo {
                    tok: Token::Eof,
                    span: Span::new(self.pos, self.pos),
                    line: self.line,
                });
            };
            match c {
                b'\n' | b'\r' => self.newline(),
                b' ' | b'\t' | 0x0B | 0x0C => self.bump(),
                b'-' if self.at(1) == Some(b'-') => {
                    self.pos += 2;
                    self.comment()?;
                }
                _ => {
                    let tok = self.token(c, start, line)?;
                    return Ok(TokenInfo {
                        tok,
                        span: Span::new(start, self.pos),
                        line,
                    });
                }
            }
        }
    }

    fn comment(&mut self) -> Result<(), SyntaxError> {
        if self.cur() == Some(b'[')
            && let Some(level) = self.long_bracket_level()
        {
            self.pos += 2 + level as usize;
            self.long_string(level, true)?;
            return Ok(());
        }
        while !matches!(self.cur(), None | Some(b'\n') | Some(b'\r')) {
            self.bump();
        }
        Ok(())
    }

    fn token(&mut self, c: u8, start: usize, line: u32) -> Result<Token, SyntaxError> {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => Ok(self.name_or_keyword()),
            b'0'..=b'9' => self.number(start, line),
            b'"' | b'\'' => self.string(c),
            b'[' => match self.long_bracket_level() {
                Some(level) => {
                    self.pos += 2 + level as usize;
                    Ok(Token::Str(self.long_string(level, false)?))
                }
                None if self.at(1) == Some(b'=') => {
                    self.bump();
                    while self.cur() == Some(b'=') {
                        self.bump();
                    }
                    Err(self.err_near(line, "invalid long string delimiter", start))
                }
                None => {
                    self.bump();
                    Ok(Token::LBracket)
                }
            },
            b'+' => {
                self.bump();
                Ok(Token::Plus)
            }
            b'-' => {
                self.bump();
                Ok(Token::Minus)
            }
            b'*' => {
                self.bump();
                Ok(Token::Star)
            }
            b'/' => {
                self.bump();
                if self.cur() == Some(b'/') && self.version.has_idiv() {
                    self.bump();
                    Ok(Token::DSlash)
                } else {
                    Ok(Token::Slash)
                }
            }
            b'%' => {
                self.bump();
                Ok(Token::Percent)
            }
            b'^' => {
                self.bump();
                Ok(Token::Caret)
            }
            b'#' => {
                self.bump();
                Ok(Token::Hash)
            }
            b'&' if self.version.has_bitwise_ops() => {
                self.bump();
                Ok(Token::Amp)
            }
            b'|' if self.version.has_bitwise_ops() => {
                self.bump();
                Ok(Token::Pipe)
            }
            b'~' => {
                self.bump();
                if self.cur() == Some(b'=') {
                    self.bump();
                    Ok(Token::Ne)
                } else if self.version.has_bitwise_ops() {
                    Ok(Token::Tilde)
                } else {
                    Err(self.err_near(line, "unexpected symbol", start))
                }
            }
            b'<' => {
                self.bump();
                match self.cur() {
                    Some(b'=') => {
                        self.bump();
                        Ok(Token::Le)
                    }
                    Some(b'<') if self.version.has_bitwise_ops() => {
                        self.bump();
                        Ok(Token::Shl)
                    }
                    _ => Ok(Token::Lt),
                }
            }
            b'>' => {
                self.bump();
                match self.cur() {
                    Some(b'=') => {
                        self.bump();
                        Ok(Token::Ge)
                    }
                    Some(b'>') if self.version.has_bitwise_ops() => {
                        self.bump();
                        Ok(Token::Shr)
                    }
                    _ => Ok(Token::Gt),
                }
            }
            b'=' => {
                self.bump();
                if self.cur() == Some(b'=') {
                    self.bump();
                    Ok(Token::Eq)
                } else {
                    Ok(Token::Assign)
                }
            }
            b'(' => {
                self.bump();
                Ok(Token::LParen)
            }
            b')' => {
                self.bump();
                Ok(Token::RParen)
            }
            b'{' => {
                self.bump();
                Ok(Token::LBrace)
            }
            b'}' => {
                self.bump();
                Ok(Token::RBrace)
            }
            b']' => {
                self.bump();
                Ok(Token::RBracket)
            }
            b';' => {
                self.bump();
                Ok(Token::Semi)
            }
            b',' => {
                self.bump();
                Ok(Token::Comma)
            }
            b':' => {
                self.bump();
                if self.cur() == Some(b':') && self.version.has_goto() {
                    self.bump();
                    Ok(Token::DColon)
                } else {
                    Ok(Token::Colon)
                }
            }
            b'.' => match self.at(1) {
                Some(b'.') => {
                    self.pos += 2;
                    if self.cur() == Some(b'.') {
                        self.bump();
                        Ok(Token::Ellipsis)
                    } else {
                        Ok(Token::Concat)
                    }
                }
                Some(b'0'..=b'9') => self.number(start, line),
                _ => {
                    self.bump();
                    Ok(Token::Dot)
                }
            },
            _ => {
                self.bump();
                Err(self.err_near(line, "unexpected symbol", start))
            }
        }
    }

    fn name_or_keyword(&mut self) -> Token {
        let start = self.pos;
        while matches!(
            self.cur(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
        ) {
            self.bump();
        }
        let text = &self.src[start..self.pos];
        match text {
            b"and" => Token::And,
            b"break" => Token::Break,
            b"do" => Token::Do,
            b"else" => Token::Else,
            b"elseif" => Token::Elseif,
            b"end" => Token::End,
            b"false" => Token::False,
            b"for" => Token::For,
            b"function" => Token::Function,
            b"global" if self.version.has_global_decl() => Token::Global,
            b"goto" if self.version.has_goto() => Token::Goto,
            b"if" => Token::If,
            b"in" => Token::In,
            b"local" => Token::Local,
            b"nil" => Token::Nil,
            b"not" => Token::Not,
            b"or" => Token::Or,
            b"repeat" => Token::Repeat,
            b"return" => Token::Return,
            b"then" => Token::Then,
            b"true" => Token::True,
            b"until" => Token::Until,
            b"while" => Token::While,
            _ => Token::Name(str::from_utf8(text).expect("ascii identifier").into()),
        }
    }

    // ---- long brackets ----

    /// At `[`: returns the level if this position opens a long bracket
    /// (`[[`, `[=[`, ...), without consuming anything.
    fn long_bracket_level(&self) -> Option<u32> {
        let mut n = 0;
        while self.at(1 + n as usize) == Some(b'=') {
            n += 1;
        }
        (self.at(1 + n as usize) == Some(b'[')).then_some(n)
    }

    /// Body of a long string/comment; opener already consumed.
    fn long_string(&mut self, level: u32, is_comment: bool) -> Result<Vec<u8>, SyntaxError> {
        let open_line = self.line;
        let mut out = Vec::new();
        // a newline right after the opening bracket is skipped
        if matches!(self.cur(), Some(b'\n' | b'\r')) {
            self.newline();
        }
        loop {
            match self.cur() {
                None => {
                    let what = if is_comment { "comment" } else { "string" };
                    return Err(self.err(
                        self.line,
                        format!(
                            "unfinished long {what} (starting at line {open_line}) near '<eof>'"
                        ),
                    ));
                }
                Some(b']') => {
                    let mut n = 0;
                    while self.at(1 + n as usize) == Some(b'=') {
                        n += 1;
                    }
                    if n == level && self.at(1 + n as usize) == Some(b']') {
                        self.pos += 2 + n as usize;
                        return Ok(out);
                    }
                    out.push(b']');
                    self.bump();
                }
                Some(b'[')
                    if !is_comment
                        && level == 0
                        && self.at(1) == Some(b'[')
                        && self.version.rejects_nested_long_string() =>
                {
                    return Err(self.err(self.line, "nesting of [[...]] is deprecated near '['"));
                }
                Some(b'\n' | b'\r') => {
                    out.push(b'\n');
                    self.newline();
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    // ---- short strings ----

    fn string(&mut self, quote: u8) -> Result<Token, SyntaxError> {
        self.bump();
        let mut out = Vec::new();
        loop {
            match self.cur() {
                None | Some(b'\n') | Some(b'\r') => {
                    return Err(self.err(self.line, "unfinished string".to_string()));
                }
                Some(c) if c == quote => {
                    self.bump();
                    return Ok(Token::Str(out));
                }
                Some(b'\\') => {
                    self.bump();
                    self.escape(&mut out)?;
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), SyntaxError> {
        let esc_start = self.pos - 1;
        let Some(c) = self.cur() else {
            return Err(self.err(self.line, "unfinished string".to_string()));
        };
        match c {
            b'a' => {
                out.push(7);
                self.bump();
            }
            b'b' => {
                out.push(8);
                self.bump();
            }
            b'f' => {
                out.push(12);
                self.bump();
            }
            b'n' => {
                out.push(b'\n');
                self.bump();
            }
            b'r' => {
                out.push(b'\r');
                self.bump();
            }
            b't' => {
                out.push(b'\t');
                self.bump();
            }
            b'v' => {
                out.push(11);
                self.bump();
            }
            b'\\' | b'"' | b'\'' => {
                out.push(c);
                self.bump();
            }
            b'\n' | b'\r' => {
                self.newline();
                out.push(b'\n');
            }
            b'x' if self.version.has_extended_escapes() => {
                self.bump();
                let mut v: u32 = 0;
                for _ in 0..2 {
                    let Some(d) = self.cur().and_then(hex_digit) else {
                        self.cur().is_some().then(|| self.bump());
                        return Err(self.err_near(
                            self.line,
                            "hexadecimal digit expected",
                            esc_start,
                        ));
                    };
                    v = v * 16 + d;
                    self.bump();
                }
                out.push(v as u8);
            }
            b'z' if self.version.has_extended_escapes() => {
                self.bump();
                loop {
                    match self.cur() {
                        Some(b'\n' | b'\r') => self.newline(),
                        Some(b' ' | b'\t' | 0x0B | 0x0C) => self.bump(),
                        _ => break,
                    }
                }
            }
            b'u' if self.version.has_extended_escapes() => {
                self.bump();
                if self.cur() != Some(b'{') {
                    return Err(self.err_near(self.line, "missing '{' in \\u{xxxx}", esc_start));
                }
                self.bump();
                let Some(d0) = self.cur().and_then(hex_digit) else {
                    return Err(self.err_near(self.line, "hexadecimal digit expected", esc_start));
                };
                let mut v: u64 = d0 as u64;
                self.bump();
                while let Some(d) = self.cur().and_then(hex_digit) {
                    v = v * 16 + d as u64;
                    if v > 0x7FFF_FFFF {
                        return Err(self.err_near(self.line, "UTF-8 value too large", esc_start));
                    }
                    self.bump();
                }
                if self.cur() != Some(b'}') {
                    return Err(self.err_near(self.line, "missing '}' in \\u{xxxx}", esc_start));
                }
                self.bump();
                push_utf8(out, v as u32);
            }
            b'0'..=b'9' => {
                let mut v: u32 = 0;
                for _ in 0..3 {
                    let Some(d @ b'0'..=b'9') = self.cur() else {
                        break;
                    };
                    v = v * 10 + (d - b'0') as u32;
                    self.bump();
                }
                if v > 255 {
                    return Err(self.err_near(self.line, "decimal escape too large", esc_start));
                }
                out.push(v as u8);
            }
            _ => {
                self.bump();
                return Err(self.err_near(self.line, "invalid escape sequence", esc_start));
            }
        }
        Ok(())
    }

    // ---- numbers ----

    /// Greedy scan (PUC-style: alphanumerics, '.', and exponent signs), then
    /// strict validation; mirrors read_numeral + lua_strtonumber.
    fn number(&mut self, start: usize, line: u32) -> Result<Token, SyntaxError> {
        let hex = self.cur() == Some(b'0') && matches!(self.at(1), Some(b'x' | b'X'));
        if hex {
            self.pos += 2;
        }
        let exp_marks: &[u8] = if hex { b"pP" } else { b"eE" };
        while let Some(c) = self.cur() {
            if exp_marks.contains(&c) {
                self.bump();
                if matches!(self.cur(), Some(b'+' | b'-')) {
                    self.bump();
                }
            } else if c.is_ascii_alphanumeric() || c == b'.' {
                self.bump();
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        let malformed = || SyntaxError {
            line,
            msg: format!("malformed number near '{}'", String::from_utf8_lossy(text)),
        };
        let tok = if hex {
            parse_hex(&text[2..], self.version)
        } else {
            parse_dec(text, self.version)
        };
        tok.ok_or_else(malformed)
    }
}

fn hex_digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// Extended UTF-8 (up to 6 bytes, values to 2^31-1), as luaO_utf8esc.
fn push_utf8(out: &mut Vec<u8>, mut x: u32) {
    if x < 0x80 {
        out.push(x as u8);
        return;
    }
    let mut cont = [0u8; 6];
    let mut n = 0;
    let mut mfb: u32 = 0x3f;
    loop {
        cont[n] = 0x80 | (x & 0x3f) as u8;
        n += 1;
        x >>= 6;
        mfb >>= 1;
        if x <= mfb {
            break;
        }
    }
    out.push(((!mfb << 1) | x) as u8);
    out.extend(cont[..n].iter().rev());
}

/// Hex numeral after the `0x` prefix.
fn parse_hex(text: &[u8], version: LuaVersion) -> Option<Token> {
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
        // pure hex integer: wraps modulo 2^64 (5.3+ semantics)
        let mut v: u64 = 0;
        for &c in &text[..int_end] {
            v = v
                .wrapping_mul(16)
                .wrapping_add(hex_digit(c).unwrap() as u64);
        }
        return Some(if version.has_integers() {
            Token::Int(v as i64)
        } else {
            Token::Float(v as f64)
        });
    }
    if !version.has_hex_float() {
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
    Some(Token::Float(compose_f64(mant, sticky, exp4 * 4 + pexp)))
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

/// Decimal numeral.
fn parse_dec(text: &[u8], version: LuaVersion) -> Option<Token> {
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
    if !has_dot && !has_exp && version.has_integers() {
        // decimal integer; on i64 overflow it becomes a float (PUC rule)
        if let Ok(v) = s.parse::<i64>() {
            return Some(Token::Int(v));
        }
    }
    s.parse::<f64>().ok().map(Token::Float)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str, v: LuaVersion) -> Result<Vec<Token>, SyntaxError> {
        let mut lex = Lexer::new(src.as_bytes(), v);
        let mut out = Vec::new();
        loop {
            let t = lex.next_token()?;
            if t.tok == Token::Eof {
                return Ok(out);
            }
            out.push(t.tok);
        }
    }

    #[test]
    fn numbers_55() {
        let v = LuaVersion::Lua55;
        assert_eq!(toks("3", v).unwrap(), vec![Token::Int(3)]);
        assert_eq!(toks("3.0", v).unwrap(), vec![Token::Float(3.0)]);
        assert_eq!(toks("345", v).unwrap(), vec![Token::Int(345)]);
        assert_eq!(toks("0xff", v).unwrap(), vec![Token::Int(255)]);
        assert_eq!(toks("0x1p4", v).unwrap(), vec![Token::Float(16.0)]);
        assert_eq!(toks("0x0.8", v).unwrap(), vec![Token::Float(0.5)]);
        assert_eq!(toks("0xA.8p1", v).unwrap(), vec![Token::Float(21.0)]);
        assert_eq!(toks(".5e2", v).unwrap(), vec![Token::Float(50.0)]);
        assert_eq!(toks("1e2", v).unwrap(), vec![Token::Float(100.0)]);
        // decimal i64 overflow becomes a float
        assert_eq!(
            toks("9223372036854775808", v).unwrap(),
            vec![Token::Float(9223372036854775808.0)]
        );
        // hex wraps modulo 2^64
        assert_eq!(toks("0xFFFFFFFFFFFFFFFF", v).unwrap(), vec![Token::Int(-1)]);
        assert!(toks("3..2", v).is_err());
        assert!(toks("3a", v).is_err());
        assert!(toks("0x", v).is_err());
        assert!(toks("1e+", v).is_err());
    }

    #[test]
    fn numbers_51() {
        let v = LuaVersion::Lua51;
        assert_eq!(toks("3", v).unwrap(), vec![Token::Float(3.0)]);
        assert_eq!(toks("0x10", v).unwrap(), vec![Token::Float(16.0)]);
        assert!(toks("0x1p4", v).is_err());
    }

    #[test]
    fn strings() {
        let v = LuaVersion::Lua55;
        assert_eq!(
            toks(r#""a\65\x42\u{48}c""#, v).unwrap(),
            vec![Token::Str(b"aABHc".to_vec())]
        );
        assert_eq!(
            toks("\"a\\z  \n  b\"", v).unwrap(),
            vec![Token::Str(b"ab".to_vec())]
        );
        assert_eq!(
            toks("[==[\nhey]]==]", v).unwrap(),
            vec![Token::Str(b"hey]".to_vec())]
        );
        assert!(toks(r#""\x4""#, v).is_err());
        assert!(toks(r#""\300""#, v).is_err());
        assert!(toks(r#""\x41""#, LuaVersion::Lua51).is_err());
    }

    #[test]
    fn version_gates() {
        assert!(
            toks("a // b", LuaVersion::Lua51).is_err() || {
                // `//` lexes as two Slash tokens in 5.1; parser rejects later
                toks("a // b", LuaVersion::Lua51)
                    .unwrap()
                    .contains(&Token::Slash)
            }
        );
        assert_eq!(
            toks("goto", LuaVersion::Lua51).unwrap(),
            vec![Token::Name("goto".into())]
        );
        assert_eq!(toks("goto", LuaVersion::Lua55).unwrap(), vec![Token::Goto]);
        assert_eq!(
            toks("global", LuaVersion::Lua54).unwrap(),
            vec![Token::Name("global".into())]
        );
        assert_eq!(
            toks("global", LuaVersion::Lua55).unwrap(),
            vec![Token::Global]
        );
        assert!(toks("a & b", LuaVersion::Lua51).is_err());
    }

    #[test]
    fn shebang_and_bom() {
        let v = LuaVersion::Lua55;
        assert_eq!(
            toks("#!/usr/bin/lua\nreturn", v).unwrap(),
            vec![Token::Return]
        );
    }
}
