//! Numerals.

use super::*;

impl<S: Source> Lexer<'_, S> {
    /// PUC `read_numeral` over the numeral starting at `start` (a leading
    /// `.` included). The
    /// scan is liberal and the conversion decides: 5.1 takes a run of
    /// digits and dots, an exponent, then any alphanumerics; 5.2+ take hex
    /// digits, dots and exponents, and 5.4+ also swallow one touching
    /// letter so `3x` is malformed instead of `3` followed by `x`.
    pub(super) fn number(&mut self, start: usize) -> Result<Tok, SyntaxError> {
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
        let text = &self.src.bytes()[start..self.pos];
        // up to 15 decimal digits: exact as a double, and no conversion of
        // them sets errno
        if text.len() <= 15 && text.iter().all(u8::is_ascii_digit) {
            let n = text.iter().fold(0u64, |a, &c| a * 10 + u64::from(c - b'0'));
            return Ok(if v.has_integers() {
                Token::Int(n as i64)
            } else {
                Token::Float(n as f64)
            });
        }
        let hex = text.len() > 1 && text[0] == b'0' && matches!(text[1], b'x' | b'X');
        {
            use crate::cerrno::conv::{Dialect, number};
            let d = match v {
                LuaVersion::Lua51 => Dialect::Lua51,
                LuaVersion::Lua52 => Dialect::Lua52,
                _ => Dialect::Later,
            };
            number(text, d);
        }
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
                self.buf.clear();
                self.buf.extend_from_slice_or_abort(text);
                let e = self.buf_error("malformed number");
                self.near_kind.set(NearKind::Numeral);
                Err(e)
            }
        }
    }
}
