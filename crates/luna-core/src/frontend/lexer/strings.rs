//! String literals: long brackets and short strings.

use super::*;

impl<'s> Lexer<'s> {
    /// PUC `skip_sep` at a `[` or `]`: saves the bracket and any `=`s, and
    /// returns the level when the same bracket follows (a well-formed
    /// opener/closer), leaving that second bracket unconsumed.
    pub(super) fn skip_sep(&mut self) -> Option<u32> {
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
    /// in the buffer, that bracket is current. Returns the string token.
    pub(super) fn long_string(&mut self, level: u32, is_comment: bool) -> Result<Tok, SyntaxError> {
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
                            return Ok(Token::Eof);
                        }
                        let n = 2 + level as usize;
                        return Ok(self.str_token(n, self.buf.len() - n));
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

    /// PUC `read_string`. The buffer starts with the delimiter; where escape
    /// bytes are kept for error messages differs per dialect (5.1 never
    /// keeps the backslash, 5.2 rebuilds the buffer from the escape on
    /// error, 5.3+ keep everything until the escape is complete).
    pub(super) fn string(&mut self, del: u8) -> Result<Tok, SyntaxError> {
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
        Ok(self.str_token(1, self.buf.len() - 1))
    }
}
