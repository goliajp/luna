//! Byte-driven lexer. The source is an arbitrary byte sequence (Lua sources
//! and string literals are not required to be UTF-8); only `\u{...}` escapes
//! produce UTF-8 output.

use crate::frontend::error::SyntaxError;
use crate::frontend::names::{Names, Sym};
use crate::frontend::span::Span;
use crate::frontend::token::{LexTok, Near, Tok, Token, TokenInfo, near_text};
use crate::numeric::{self, Num, hex_digit};
use crate::version::LuaVersion;

mod escape;
mod interning;
mod strings;

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
    /// set for the load path: identifiers and string literals are interned
    /// here and handed out as `last_sym` with an empty `Token::Name` /
    /// `Token::Str`
    names: Option<Names>,
    /// the number of the identifier or literal just read, when interning
    pub(crate) last_sym: Sym,
    /// where the contents of the string literal just read are in `buf`,
    /// when not interning
    str_range: (usize, usize),
}

/// One lexed item as the parser sees it: either a token, or a byte PUC's
/// scanner hands back as a single-character token of its own (`@`, `$`,
/// `&` before 5.3, ...). Those are only an error once the parser finds no
/// use for them, and what it then says depends on where they appear.
pub(crate) enum Lexed {
    Tok(LexTok),
    Char(u8, LexTok),
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
            names: None,
            last_sym: Sym(0),
            str_range: (0, 0),
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
            Lexed::Tok(t) => Ok(self.token_info(t)),
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
                return Ok(Lexed::Tok(LexTok {
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
                    let info = |tok| LexTok {
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
    fn token(&mut self, c: u8) -> Result<Result<Tok, u8>, SyntaxError> {
        let v = self.version;
        let tok = match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.name_or_keyword(),
            b'0'..=b'9' => self.number(self.pos)?,
            b'"' | b'\'' => self.string(c)?,
            b'[' => match self.skip_sep() {
                Some(level) => self.long_string(level, false)?,
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

    fn name_or_keyword(&mut self) -> Tok {
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
            _ => {
                if let Some(names) = &mut self.names {
                    self.last_sym = names.intern(text);
                }
                Token::Name(())
            }
        }
    }

    /// PUC `read_numeral` over the numeral starting at `start` (a leading
    /// `.` included). The
    /// scan is liberal and the conversion decides: 5.1 takes a run of
    /// digits and dots, an exponent, then any alphanumerics; 5.2+ take hex
    /// digits, dots and exponents, and 5.4+ also swallow one touching
    /// letter so `3x` is malformed instead of `3` followed by `x`.
    fn number(&mut self, start: usize) -> Result<Tok, SyntaxError> {
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

#[cfg(test)]
mod tests;
