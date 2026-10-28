//! One token from its first byte: names and reserved words, numerals,
//! strings, and the punctuation of each dialect.

use super::*;

impl<S: Source> Lexer<'_, S> {
    /// One token starting at byte `c`. `Err(byte)` is a byte PUC returns as
    /// a single-character token that no Lua syntax uses.
    pub(super) fn token(&mut self, c: u8) -> Result<Result<Tok, u8>, SyntaxError> {
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
        // the name's bytes read in place; the last one is left by `bump`,
        // which reads a stream's next piece past it
        let bytes = self.src.bytes();
        let mut p = start + 1;
        while p < bytes.len() && matches!(bytes[p], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
        {
            p += 1;
        }
        self.pos = p - 1;
        self.bump();
        while S::STREAMS
            && matches!(
                self.cur(),
                Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
            )
        {
            self.bump();
        }
        let text = &self.src.bytes()[start..self.pos];
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
                    self.last_sym = names.intern_ascii(text);
                }
                Token::Name(())
            }
        }
    }
}
