//! Byte-driven lexer. The source is an arbitrary byte sequence (Lua sources
//! and string literals are not required to be UTF-8); only `\u{...}` escapes
//! produce UTF-8 output.

use crate::frontend::error::SyntaxError;
use crate::frontend::span::Span;
use crate::frontend::token::{Token, TokenInfo};
use crate::numeric::{self, Num, hex_digit};
use crate::version::LuaVersion;

/// Streaming Lua lexer. Holds a borrowed reference to the source bytes and
/// the current line counter; `next_token()` produces one [`TokenInfo`] at a
/// time.
pub struct Lexer<'s> {
    src: &'s [u8],
    pos: usize,
    line: u32,
    version: LuaVersion,
}

impl<'s> Lexer<'s> {
    /// Build a lexer over `src` for the given Lua dialect.
    pub fn new(src: &'s [u8], version: LuaVersion) -> Lexer<'s> {
        Lexer {
            src,
            pos: 0,
            line: 1,
            version,
        }
    }

    /// Borrow the source bytes the lexer is iterating.
    pub fn src(&self) -> &'s [u8] {
        self.src
    }

    /// Strip a leading UTF-8 BOM and `#...` shebang line from a *file* chunk,
    /// as PUC's `luaL_loadfilex` does. String `load()` never strips these, so
    /// this is applied by the file loaders only — not in the lexer itself. The
    /// terminating newline is left in place so line numbers count the shebang
    /// line as line 1.
    pub fn strip_shebang_bom(src: &[u8]) -> &[u8] {
        let mut p = 0;
        if src.starts_with(&[0xEF, 0xBB, 0xBF]) {
            p = 3;
        }
        if src.get(p) == Some(&b'#') {
            while !matches!(src.get(p), None | Some(b'\n') | Some(b'\r')) {
                p += 1;
            }
        }
        &src[p..]
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
            msg: msg.into().into_bytes(),
        }
    }

    /// Like `err` but accepts a `Vec<u8>` directly, for the lexer's raw-byte
    /// near-token paths (PUC 5.1 `near '\xff'` cases need the offending byte
    /// in the message verbatim, which a `String` round-trip would garble).
    fn err_bytes(&self, line: u32, msg: Vec<u8>) -> SyntaxError {
        SyntaxError { line, msg }
    }

    fn err_near(&self, line: u32, msg: &str, start: usize) -> SyntaxError {
        // PUC `luaX_token2str` rendered a non-printable single byte through
        // three successive forms:
        //   - 5.1: `iscntrl` → `char(N)` (unwrapped), otherwise the raw byte
        //          (`\xff` fails `iscntrl` in the C locale).
        //   - 5.2: `char(N)` for any non-printable byte, unwrapped.
        //   - 5.3+: `'<\N>'` (decimal, single-quoted by `luaX_token2str`).
        // errors.lua exercises each: 5.1 :196 raw, 5.2 :352 `char(255)`,
        // 5.3 :461 / 5.4 :620 `<\255>`. `SyntaxError.msg` is `Vec<u8>` so
        // the 5.1 raw-byte branch can carry `\xff` verbatim — `errors.lua`
        // 5.1 :20's `checksyntax([[\xffa = 1]], …, "\xff", 1)` pattern
        // `near '%\xff'` then matches.
        let bytes = &self.src[start..self.pos];
        if bytes.len() == 1 && !bytes[0].is_ascii_graphic() {
            let b = bytes[0];
            // is_ascii_control covers C0 controls (0x00..0x1f, 0x7f). 5.1
            // routes those through `char(N)` and other high-bit bytes
            // through the raw form.
            let raw_byte_form = self.version <= LuaVersion::Lua51 && !b.is_ascii_control();
            if self.version >= LuaVersion::Lua53 {
                return self.err(line, format!("{msg} near '<\\{b}>'"));
            }
            if raw_byte_form {
                let mut out = Vec::with_capacity(msg.len() + 6);
                out.extend_from_slice(msg.as_bytes());
                out.extend_from_slice(b" near '");
                out.push(b);
                out.push(b'\'');
                return self.err_bytes(line, out);
            }
            return self.err(line, format!("{msg} near char({b})"));
        }
        let text = String::from_utf8_lossy(bytes).into_owned();
        self.err(line, format!("{msg} near '{text}'"))
    }

    /// Error inside a string literal: the near-token is the raw source of the
    /// string contents read so far (`content_start..pos`), mirroring PUC's
    /// `txtToken` over the lex buffer.
    ///
    /// `consume_current = true` mirrors PUC `esccheck`'s pattern of saving the
    /// offending byte into the buffer before raising (e.g. `\g` — the bad
    /// escape *letter* is part of the report). For errors that fire after the
    /// escape has been fully consumed (e.g. `\999` — overflow checked after
    /// the third digit) the caller passes `false` so the trailing string-
    /// delimiter doesn't sneak into the report.
    fn str_err(
        &mut self,
        line: u32,
        msg: &str,
        content_start: usize,
        consume_current: bool,
    ) -> SyntaxError {
        if consume_current && self.cur().is_some() {
            self.bump();
        }
        let text = String::from_utf8_lossy(&self.src[content_start..self.pos]);
        self.err(line, format!("{msg} near '{text}'"))
    }

    /// Lex the next token. Returns `Token::Eof` (with the final source line)
    /// at end-of-input; returns a [`SyntaxError`] on malformed input.
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
                // MacroLua: `}@` closes a `@{ ... }@` explicit quote block.
                // PUC 5.1-5.5 always reads a plain `}` here.
                if self.version.is_macro_lua() && self.cur() == Some(b'@') {
                    self.bump();
                    Ok(Token::MacroBraceClose)
                } else {
                    Ok(Token::RBrace)
                }
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
            // MacroLua (v1.3 Phase ML): `@` introduces a macro invocation
            // (`@name(args)` / `@quote{...}`) or an explicit quote-block
            // opener `@{`. PUC 5.1-5.5 falls through to the catch-all and
            // errors `unexpected symbol near '@'` exactly as before.
            b'@' if self.version.is_macro_lua() => {
                self.bump();
                if self.cur() == Some(b'{') {
                    self.bump();
                    Ok(Token::MacroBraceOpen)
                } else {
                    Ok(Token::At)
                }
            }
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
            // `global` is a *contextual* keyword in 5.5, not a reserved word:
            // it is only a declaration when it leads a statement (decided by
            // the parser via lookahead). Lexed as an ordinary name so uses like
            // `global = 1` / `return global` stay valid.
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
                        format!("unfinished long {what} (starting at line {open_line}) near <eof>"),
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
        let content_start = self.pos;
        let mut out = Vec::new();
        loop {
            match self.cur() {
                None | Some(b'\n') | Some(b'\r') => {
                    return Err(self.err(self.line, "unfinished string near <eof>"));
                }
                Some(c) if c == quote => {
                    self.bump();
                    return Ok(Token::Str(out));
                }
                Some(b'\\') => {
                    // PUC `escerror` resets the lex buffer to just the
                    // offending `\<esc>` before raising for `\x` / decimal
                    // escapes — so the "near 'X'" suffix only quotes the
                    // escape, not the whole string body. `\u{…}` errors keep
                    // the full buffer (PUC's `utf8esc` calls `esccheck`
                    // without the buffer reset). `escape` gets both starts
                    // and picks the right one per error.
                    let esc_start = self.pos;
                    self.bump();
                    self.escape(&mut out, content_start, esc_start)?;
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn escape(
        &mut self,
        out: &mut Vec<u8>,
        full_start: usize,
        esc_start: usize,
    ) -> Result<(), SyntaxError> {
        // `esc_start` clips the near-token to just the offending escape
        // (matches PUC's `escerror` for `\x` / decimal); `full_start` keeps
        // the whole string body in the report (matches PUC's `\u{…}` path).
        let content_start = esc_start;
        let _ = full_start;
        let Some(c) = self.cur() else {
            return Err(self.err(self.line, "unfinished string near <eof>"));
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
                        return Err(self.str_err(
                            self.line,
                            "hexadecimal digit expected",
                            content_start,
                            true,
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
                    return Err(self.str_err(
                        self.line,
                        "missing '{' in \\u{xxxx}",
                        full_start,
                        true,
                    ));
                }
                self.bump();
                let Some(d0) = self.cur().and_then(hex_digit) else {
                    return Err(self.str_err(
                        self.line,
                        "hexadecimal digit expected",
                        full_start,
                        true,
                    ));
                };
                let mut v: u64 = d0 as u64;
                self.bump();
                // PUC 5.3 caps \u escapes at the Unicode max 0x10FFFF; 5.4
                // widened the lexer cap to 0x7FFFFFFF (extended UTF-8). The
                // shift-then-check pattern matches PUC's loop so the offending
                // digit becomes the near token's last char.
                let cap = if self.version >= LuaVersion::Lua54 {
                    0x07FF_FFFFu64
                } else {
                    0x0010_FFFFu64 / 16
                };
                while let Some(d) = self.cur().and_then(hex_digit) {
                    if v > cap {
                        return Err(self.str_err(
                            self.line,
                            "UTF-8 value too large",
                            full_start,
                            true,
                        ));
                    }
                    v = v * 16 + d as u64;
                    self.bump();
                }
                if v > if self.version >= LuaVersion::Lua54 {
                    0x7FFF_FFFF
                } else {
                    0x0010_FFFF
                } {
                    return Err(self.str_err(self.line, "UTF-8 value too large", full_start, true));
                }
                if self.cur() != Some(b'}') {
                    return Err(self.str_err(
                        self.line,
                        "missing '}' in \\u{xxxx}",
                        full_start,
                        true,
                    ));
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
                    // PUC 5.2's `escerror` reports the escape only (`\999`);
                    // 5.3+ extended it to also include the byte that follows
                    // (`\999"`). The literals.lua expectation flipped with the
                    // PUC change, so we mirror it per-dialect.
                    let consume = self.version >= LuaVersion::Lua53;
                    return Err(self.str_err(
                        self.line,
                        "decimal escape too large",
                        content_start,
                        consume,
                    ));
                }
                out.push(v as u8);
            }
            _ => {
                return Err(self.str_err(
                    self.line,
                    "invalid escape sequence",
                    content_start,
                    true,
                ));
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
            msg: format!("malformed number near '{}'", String::from_utf8_lossy(text)).into_bytes(),
        };
        let int_ok = self.version.has_integers();
        let num = if hex {
            numeric::hex_literal(&text[2..], int_ok, self.version.has_hex_float())
        } else {
            // a numeric literal carries no sign (unary minus is a separate
            // operator), so the magnitude 2^63 stays a float here
            numeric::dec_literal(text, int_ok, false)
        };
        match num {
            Some(Num::Int(i)) => Ok(Token::Int(i)),
            Some(Num::Float(f)) => Ok(Token::Float(f)),
            None => Err(malformed()),
        }
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
        // `global` is a contextual keyword (parser decides); the lexer always
        // produces a plain name in every version.
        assert_eq!(
            toks("global", LuaVersion::Lua54).unwrap(),
            vec![Token::Name("global".into())]
        );
        assert_eq!(
            toks("global", LuaVersion::Lua55).unwrap(),
            vec![Token::Name("global".into())]
        );
        assert!(toks("a & b", LuaVersion::Lua51).is_err());
    }

    #[test]
    fn shebang_and_bom() {
        // shebang/BOM stripping is a file-load concern, not the lexer's: the
        // helper removes them, leaving the newline so line counts are kept.
        assert_eq!(
            Lexer::strip_shebang_bom(b"#!/usr/bin/lua\nreturn"),
            b"\nreturn"
        );
        assert_eq!(Lexer::strip_shebang_bom(&[0xEF, 0xBB, 0xBF, b'x']), b"x");
        // a string chunk keeps `#` as the length operator (no stripping here)
        let v = LuaVersion::Lua55;
        assert_eq!(
            toks("#a", v).unwrap(),
            vec![Token::Hash, Token::Name("a".into())]
        );
    }
}
