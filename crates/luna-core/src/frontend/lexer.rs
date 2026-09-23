//! Byte-driven lexer. The source is an arbitrary byte sequence (Lua sources
//! and string literals are not required to be UTF-8); only `\u{...}` escapes
//! produce UTF-8 output.

use crate::frontend::error::SyntaxError;
use crate::frontend::span::Span;
use crate::frontend::token::{Near, Token, TokenInfo, near_text};
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
    /// PUC's `ls->buff`: the text of the token being scanned. Error messages
    /// quote it as the near-token, so it is kept in the exact shape each
    /// dialect's scanner leaves it in (escapes half-decoded, delimiters
    /// kept, and so on).
    buf: Vec<u8>,
}

/// One lexed item as the parser sees it: either a token, or a byte PUC's
/// scanner hands back as a single-character token of its own (`@`, `$`,
/// `&` before 5.3, ...). Those are only an error once the parser finds no
/// use for them, and what it then says depends on where they appear.
pub(crate) enum Lexed {
    Tok(TokenInfo),
    Char(u8, TokenInfo),
}

impl<'s> Lexer<'s> {
    /// Build a lexer over `src` for the given Lua dialect.
    pub fn new(src: &'s [u8], version: LuaVersion) -> Lexer<'s> {
        Lexer {
            src,
            pos: 0,
            line: 1,
            version,
            buf: Vec::new(),
        }
    }

    /// Borrow the source bytes the lexer is iterating.
    pub fn src(&self) -> &'s [u8] {
        self.src
    }

    /// The line the scanner is on (PUC `ls->linenumber`): the line where
    /// the most recently read token ends. Syntax errors are reported here.
    pub fn line(&self) -> u32 {
        self.line
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

    fn save(&mut self, c: u8) {
        self.buf.push(c);
    }

    /// Save the current byte and advance (PUC `save_and_next`).
    fn save_next(&mut self) {
        if let Some(c) = self.cur() {
            self.buf.push(c);
        }
        self.bump();
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

    fn cur_is_newline(&self) -> bool {
        matches!(self.cur(), Some(b'\n' | b'\r'))
    }

    /// PUC `lexerror`: `msg near <token>` at the scanner's current line.
    fn error(&self, msg: &str, near: Near<'_>) -> SyntaxError {
        let mut out = msg.as_bytes().to_vec();
        out.extend_from_slice(b" near ");
        out.extend_from_slice(&near_text(self.version, near));
        SyntaxError {
            line: self.line,
            msg: out,
        }
    }

    /// A lexer error quoting the lex buffer.
    fn buf_error(&self, msg: &str) -> SyntaxError {
        self.error(msg, Near::Text(&self.buf))
    }

    /// Lex the next token. Returns `Token::Eof` (with the final source line)
    /// at end-of-input; returns a [`SyntaxError`] on malformed input,
    /// including a byte no token starts with.
    pub fn next_token(&mut self) -> Result<TokenInfo, SyntaxError> {
        match self.next_lexed()? {
            Lexed::Tok(t) => Ok(t),
            // PUC's token code for a NUL byte is 0, which `lexerror` takes
            // as "no near-token".
            Lexed::Char(0, _) => Err(SyntaxError::new(self.line, "unexpected symbol")),
            Lexed::Char(c, _) => Err(self.error("unexpected symbol", Near::Char(c))),
        }
    }

    /// Like [`Lexer::next_token`], but hands an unrecognised byte back to
    /// the caller instead of failing on it (PUC `llex`'s default case).
    pub(crate) fn next_lexed(&mut self) -> Result<Lexed, SyntaxError> {
        self.buf.clear();
        loop {
            let start = self.pos;
            let line = self.line;
            let Some(c) = self.cur() else {
                return Ok(Lexed::Tok(TokenInfo {
                    tok: Token::Eof,
                    span: Span::new(self.pos, self.pos),
                    line: self.line,
                }));
            };
            match c {
                b'\n' | b'\r' => self.newline(),
                b' ' | b'\t' | 0x0B | 0x0C => self.bump(),
                b'-' if self.at(1) == Some(b'-') => {
                    self.pos += 2;
                    self.comment()?;
                }
                _ => {
                    let tok = self.token(c)?;
                    let info = |tok| TokenInfo {
                        tok,
                        span: Span::new(start, self.pos),
                        line,
                    };
                    return Ok(match tok {
                        Ok(tok) => Lexed::Tok(info(tok)),
                        Err(c) => Lexed::Char(c, info(Token::Eof)),
                    });
                }
            }
        }
    }

    fn comment(&mut self) -> Result<(), SyntaxError> {
        if self.cur() == Some(b'[') {
            let sep = self.skip_sep();
            self.buf.clear();
            if let Some(level) = sep {
                self.long_string(level, true)?;
                self.buf.clear();
                return Ok(());
            }
        }
        while !matches!(self.cur(), None | Some(b'\n') | Some(b'\r')) {
            self.bump();
        }
        Ok(())
    }

    /// One token starting at byte `c`. `Err(byte)` is a byte PUC returns as
    /// a single-character token that no Lua syntax uses.
    fn token(&mut self, c: u8) -> Result<Result<Token, u8>, SyntaxError> {
        let v = self.version;
        let tok = match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.name_or_keyword(),
            b'0'..=b'9' => self.number(self.pos)?,
            b'"' | b'\'' => self.string(c)?,
            b'[' => match self.skip_sep() {
                Some(level) => Token::Str(self.long_string(level, false)?),
                None if self.buf.len() == 1 => Token::LBracket,
                None => return Err(self.buf_error("invalid long string delimiter")),
            },
            b'.' => {
                self.bump();
                if self.cur() == Some(b'.') {
                    self.bump();
                    if self.cur() == Some(b'.') {
                        self.bump();
                        Token::Ellipsis
                    } else {
                        Token::Concat
                    }
                } else if self.cur().is_some_and(|d| d.is_ascii_digit()) {
                    self.number(self.pos - 1)?
                } else {
                    Token::Dot
                }
            }
            _ => {
                self.bump();
                let next = self.cur();
                let mut two = |tok| {
                    self.bump();
                    tok
                };
                match (c, next) {
                    (b'=', Some(b'=')) => two(Token::Eq),
                    (b'<', Some(b'=')) => two(Token::Le),
                    (b'>', Some(b'=')) => two(Token::Ge),
                    (b'~', Some(b'=')) => two(Token::Ne),
                    (b'<', Some(b'<')) if v.has_bitwise_ops() => two(Token::Shl),
                    (b'>', Some(b'>')) if v.has_bitwise_ops() => two(Token::Shr),
                    (b'/', Some(b'/')) if v.has_idiv() => two(Token::DSlash),
                    (b':', Some(b':')) if v.has_goto() => two(Token::DColon),
                    // MacroLua: `}@` closes a `@{ ... }@` quote block and
                    // `@{` opens one; a bare `@` introduces a macro call.
                    (b'}', Some(b'@')) if v.is_macro_lua() => two(Token::MacroBraceClose),
                    (b'@', Some(b'{')) if v.is_macro_lua() => two(Token::MacroBraceOpen),
                    (b'@', _) if v.is_macro_lua() => Token::At,
                    (b'=', _) => Token::Assign,
                    (b'<', _) => Token::Lt,
                    (b'>', _) => Token::Gt,
                    (b'/', _) => Token::Slash,
                    (b':', _) => Token::Colon,
                    (b'~', _) if v.has_bitwise_ops() => Token::Tilde,
                    (b'&', _) if v.has_bitwise_ops() => Token::Amp,
                    (b'|', _) if v.has_bitwise_ops() => Token::Pipe,
                    (b'+', _) => Token::Plus,
                    (b'-', _) => Token::Minus,
                    (b'*', _) => Token::Star,
                    (b'%', _) => Token::Percent,
                    (b'^', _) => Token::Caret,
                    (b'#', _) => Token::Hash,
                    (b'(', _) => Token::LParen,
                    (b')', _) => Token::RParen,
                    (b'{', _) => Token::LBrace,
                    (b'}', _) => Token::RBrace,
                    (b']', _) => Token::RBracket,
                    (b';', _) => Token::Semi,
                    (b',', _) => Token::Comma,
                    _ => return Ok(Err(c)),
                }
            }
        };
        Ok(Ok(tok))
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

    /// PUC `skip_sep` at a `[` or `]`: saves the bracket and any `=`s, and
    /// returns the level when the same bracket follows (a well-formed
    /// opener/closer), leaving that second bracket unconsumed.
    fn skip_sep(&mut self) -> Option<u32> {
        let s = self.cur();
        self.save_next();
        let mut count = 0;
        while self.cur() == Some(b'=') {
            self.save_next();
            count += 1;
        }
        (self.cur() == s).then_some(count)
    }

    /// Body of a long string/comment; the opener up to its second bracket is
    /// in the buffer, that bracket is current. Returns the contents.
    fn long_string(&mut self, level: u32, is_comment: bool) -> Result<Vec<u8>, SyntaxError> {
        let open_line = self.line;
        self.save_next();
        if self.cur_is_newline() {
            self.newline();
        }
        loop {
            match self.cur() {
                None => {
                    let what = if is_comment { "comment" } else { "string" };
                    let msg = if self.version >= LuaVersion::Lua53 {
                        format!("unfinished long {what} (starting at line {open_line})")
                    } else {
                        format!("unfinished long {what}")
                    };
                    return Err(self.error(&msg, Near::Eof));
                }
                Some(b']') => {
                    if self.skip_sep() == Some(level) {
                        self.save_next();
                        if is_comment {
                            return Ok(Vec::new());
                        }
                        let n = 2 + level as usize;
                        return Ok(self.buf[n..self.buf.len() - n].to_vec());
                    }
                }
                // 5.1 (LUA_COMPAT_LSTR == 1) rejects a nested `[[` inside a
                // level-0 bracket, in comments too.
                Some(b'[') if self.version.rejects_nested_long_string() => {
                    if self.skip_sep() == Some(level) {
                        self.save_next();
                        if level == 0 {
                            return Err(
                                self.error("nesting of [[...]] is deprecated", Near::Char(b'['))
                            );
                        }
                    }
                }
                Some(b'\n' | b'\r') => {
                    self.save(b'\n');
                    self.newline();
                    if is_comment {
                        self.buf.clear();
                    }
                }
                Some(_) => {
                    if is_comment {
                        self.bump();
                    } else {
                        self.save_next();
                    }
                }
            }
        }
    }

    // ---- short strings ----

    /// PUC `read_string`. The buffer starts with the delimiter; where escape
    /// bytes are kept for error messages differs per dialect (5.1 never
    /// keeps the backslash, 5.2 rebuilds the buffer from the escape on
    /// error, 5.3+ keep everything until the escape is complete).
    fn string(&mut self, del: u8) -> Result<Token, SyntaxError> {
        self.save_next();
        while self.cur() != Some(del) {
            match self.cur() {
                None => return Err(self.error("unfinished string", Near::Eof)),
                Some(b'\n' | b'\r') => return Err(self.buf_error("unfinished string")),
                Some(b'\\') => match self.version {
                    LuaVersion::Lua51 => self.escape_51()?,
                    LuaVersion::Lua52 => self.escape_52()?,
                    _ => self.escape_53()?,
                },
                Some(_) => self.save_next(),
            }
        }
        self.save_next();
        Ok(Token::Str(self.buf[1..self.buf.len() - 1].to_vec()))
    }

    /// Escape letters common to every dialect; `None` for anything else.
    fn simple_escape(c: u8) -> Option<u8> {
        Some(match c {
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            _ => return None,
        })
    }

    /// 5.1: an unknown escape stands for the character itself, so `\x`,
    /// `\z` and `\u` are just `x`, `z` and `u`.
    fn escape_51(&mut self) -> Result<(), SyntaxError> {
        self.bump();
        match self.cur() {
            None => {}
            Some(b'\n' | b'\r') => {
                self.save(b'\n');
                self.newline();
            }
            Some(c) if c.is_ascii_digit() => {
                let v = self.dec_digits(|_, _| {});
                if v > 255 {
                    return Err(self.buf_error("escape sequence too large"));
                }
                self.save(v as u8);
            }
            Some(c) => {
                self.save(Self::simple_escape(c).unwrap_or(c));
                self.bump();
            }
        }
        Ok(())
    }

    /// Read up to three decimal digits, reporting each to `seen`.
    fn dec_digits(&mut self, mut seen: impl FnMut(&mut Self, u8)) -> u32 {
        let mut v = 0;
        for _ in 0..3 {
            let Some(d @ b'0'..=b'9') = self.cur() else {
                break;
            };
            v = v * 10 + (d - b'0') as u32;
            seen(self, d);
            self.bump();
        }
        v
    }

    /// 5.2 `escerror`: the buffer is replaced by `\` plus the escape's bytes.
    fn esc_error_52(&mut self, bytes: &[u8], msg: &str) -> SyntaxError {
        self.buf.clear();
        self.buf.push(b'\\');
        self.buf.extend_from_slice(bytes);
        self.buf_error(msg)
    }

    fn escape_52(&mut self) -> Result<(), SyntaxError> {
        self.bump();
        let Some(c) = self.cur() else {
            return Ok(());
        };
        if let Some(e) = Self::simple_escape(c) {
            self.save(e);
            self.bump();
            return Ok(());
        }
        match c {
            b'x' => {
                let mut seen = vec![b'x'];
                let mut v = 0;
                for _ in 0..2 {
                    self.bump();
                    let d = self.cur();
                    if let Some(d) = d {
                        seen.push(d);
                    }
                    let Some(h) = d.and_then(hex_digit) else {
                        return Err(self.esc_error_52(&seen, "hexadecimal digit expected"));
                    };
                    v = v * 16 + h;
                }
                self.bump();
                self.save(v as u8);
            }
            b'\n' | b'\r' => {
                self.newline();
                self.save(b'\n');
            }
            b'\\' | b'"' | b'\'' => {
                self.save(c);
                self.bump();
            }
            b'z' => {
                self.bump();
                self.skip_spaces();
            }
            b'0'..=b'9' => {
                let mut seen = Vec::new();
                let v = self.dec_digits(|_, d| seen.push(d));
                if v > 255 {
                    return Err(self.esc_error_52(&seen, "decimal escape too large"));
                }
                self.save(v as u8);
            }
            _ => return Err(self.esc_error_52(&[c], "invalid escape sequence")),
        }
        Ok(())
    }

    /// `\z`: skip whitespace, counting lines.
    fn skip_spaces(&mut self) {
        loop {
            match self.cur() {
                Some(b'\n' | b'\r') => self.newline(),
                Some(b' ' | b'\t' | 0x0B | 0x0C) => self.bump(),
                _ => break,
            }
        }
    }

    /// 5.3+ `esccheck`: on failure the offending byte joins the buffer so
    /// the message shows it.
    fn esc_check(&mut self, ok: bool, msg: &str) -> Result<(), SyntaxError> {
        if ok {
            return Ok(());
        }
        if self.cur().is_some() {
            self.save_next();
        }
        Err(self.buf_error(msg))
    }

    /// 5.3+ `gethexa`: save the byte before, then demand a hex digit.
    fn get_hexa(&mut self) -> Result<u32, SyntaxError> {
        self.save_next();
        let d = self.cur().and_then(hex_digit);
        self.esc_check(d.is_some(), "hexadecimal digit expected")?;
        Ok(d.expect("checked above"))
    }

    fn drop_saved(&mut self, n: usize) {
        self.buf.truncate(self.buf.len() - n);
    }

    fn escape_53(&mut self) -> Result<(), SyntaxError> {
        self.save_next();
        let Some(c) = self.cur() else {
            return Ok(());
        };
        let byte = match c {
            b'x' => {
                let r = (self.get_hexa()? << 4) + self.get_hexa()?;
                self.drop_saved(2);
                self.bump();
                r as u8
            }
            b'u' => {
                let v = self.utf8_escape()?;
                push_utf8(&mut self.buf, v);
                return Ok(());
            }
            b'\n' | b'\r' => {
                self.newline();
                b'\n'
            }
            b'\\' | b'"' | b'\'' => {
                self.bump();
                c
            }
            b'z' => {
                self.drop_saved(1);
                self.bump();
                self.skip_spaces();
                return Ok(());
            }
            _ => match Self::simple_escape(c) {
                Some(e) => {
                    self.bump();
                    e
                }
                None => {
                    self.esc_check(c.is_ascii_digit(), "invalid escape sequence")?;
                    let mut n = 0;
                    let v = self.dec_digits(|lx, d| {
                        lx.save(d);
                        n += 1;
                    });
                    self.esc_check(v <= 255, "decimal escape too large")?;
                    self.drop_saved(n);
                    v as u8
                }
            },
        };
        self.drop_saved(1);
        self.save(byte);
        Ok(())
    }

    /// 5.3+ `readutf8esc`, current at `u`; leaves the buffer as it found it
    /// minus the backslash. 5.3 caps the value at 0x10FFFF after adding each
    /// digit; 5.4 widened it to 2^31 and checks before shifting.
    fn utf8_escape(&mut self) -> Result<u32, SyntaxError> {
        let mut saved = 4;
        self.save_next();
        self.esc_check(self.cur() == Some(b'{'), "missing '{'")?;
        let mut r = self.get_hexa()?;
        loop {
            self.save_next();
            let Some(d) = self.cur().and_then(hex_digit) else {
                break;
            };
            saved += 1;
            if self.version >= LuaVersion::Lua54 {
                self.esc_check(r <= 0x7FFF_FFFF >> 4, "UTF-8 value too large")?;
                r = (r << 4) + d;
            } else {
                r = (r << 4) + d;
                self.esc_check(r <= 0x10FFFF, "UTF-8 value too large")?;
            }
        }
        self.esc_check(self.cur() == Some(b'}'), "missing '}'")?;
        self.bump();
        self.drop_saved(saved);
        Ok(r)
    }

    // ---- numbers ----

    /// PUC `read_numeral` over the numeral starting at `start` (a leading
    /// `.` included). The
    /// scan is liberal and the conversion decides: 5.1 takes a run of
    /// digits and dots, an exponent, then any alphanumerics; 5.2+ take hex
    /// digits, dots and exponents, and 5.4+ also swallow one touching
    /// letter so `3x` is malformed instead of `3` followed by `x`.
    fn number(&mut self, start: usize) -> Result<Token, SyntaxError> {
        let v = self.version;
        if v <= LuaVersion::Lua51 {
            while self.cur().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
                self.bump();
            }
            if matches!(self.cur(), Some(b'e' | b'E')) {
                self.bump();
                if matches!(self.cur(), Some(b'+' | b'-')) {
                    self.bump();
                }
            }
            while self
                .cur()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                self.bump();
            }
        } else {
            let first = self.cur();
            self.bump();
            let mut expo: &[u8] = b"eE";
            if first == Some(b'0') && matches!(self.cur(), Some(b'x' | b'X')) {
                self.bump();
                expo = b"pP";
            }
            loop {
                let c = self.cur();
                if c.is_some_and(|c| expo.contains(&c)) {
                    self.bump();
                    if matches!(self.cur(), Some(b'+' | b'-')) {
                        self.bump();
                    }
                } else if c.is_some_and(|c| c.is_ascii_hexdigit() || c == b'.') {
                    self.bump();
                } else {
                    break;
                }
            }
            if v >= LuaVersion::Lua54
                && self
                    .cur()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            {
                self.bump();
            }
        }
        // the lex buffer of a numeral is its source text
        let text = &self.src[start..self.pos];
        let hex = text.len() > 1 && text[0] == b'0' && matches!(text[1], b'x' | b'X');
        let num = if hex {
            // 5.1 converts with C99 `strtod`, which reads hex floats too.
            let float_ok = v <= LuaVersion::Lua51 || v.has_hex_float();
            numeric::hex_literal(&text[2..], v.has_integers(), float_ok)
        } else {
            // a numeric literal carries no sign (unary minus is a separate
            // operator), so the magnitude 2^63 stays a float here
            numeric::dec_literal(text, v.has_integers(), false)
        };
        match num {
            Some(Num::Int(i)) => Ok(Token::Int(i)),
            Some(Num::Float(f)) => Ok(Token::Float(f)),
            None => {
                self.buf = text.to_vec();
                Err(self.buf_error("malformed number"))
            }
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
        // PUC 5.1 converts numerals with C99 `strtod`, which reads hex floats
        assert_eq!(toks("0x1p4", v).unwrap(), vec![Token::Float(16.0)]);
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
        // 5.1 has no `\x`: an unknown escape is the character itself
        assert_eq!(
            toks(r#""\x41""#, LuaVersion::Lua51).unwrap(),
            vec![Token::Str(b"x41".to_vec())]
        );
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
