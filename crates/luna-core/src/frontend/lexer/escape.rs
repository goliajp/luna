//! Escape sequences in short strings, each dialect's way.

use super::*;

impl<S: Source> Lexer<'_, S> {
    /// Escape letters common to every dialect; `None` for anything else.
    pub(super) fn simple_escape(c: u8) -> Option<u8> {
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
    pub(super) fn escape_51(&mut self) -> Result<(), SyntaxError> {
        self.bump();
        match self.cur() {
            None => {}
            Some(b'\n' | b'\r') => {
                self.save(b'\n')?;
                self.newline();
            }
            Some(c) if c.is_ascii_digit() => {
                let v = self.dec_digits(|_, _| Ok(()))?;
                if v > 255 {
                    return Err(self.buf_error("escape sequence too large"));
                }
                self.save(v as u8)?;
            }
            Some(c) => {
                self.save(Self::simple_escape(c).unwrap_or(c))?;
                self.bump();
            }
        }
        Ok(())
    }

    /// Read up to three decimal digits, reporting each to `seen`.
    pub(super) fn dec_digits(
        &mut self,
        mut seen: impl FnMut(&mut Self, u8) -> Result<(), Oom>,
    ) -> Result<u32, Oom> {
        let mut v = 0;
        for _ in 0..3 {
            let Some(d @ b'0'..=b'9') = self.cur() else {
                break;
            };
            v = v * 10 + (d - b'0') as u32;
            seen(self, d)?;
            self.bump();
        }
        Ok(v)
    }

    /// 5.2 `escerror`: the buffer is replaced by `\` plus the escape's bytes.
    pub(super) fn esc_error_52(&mut self, bytes: &[u8], msg: &str) -> SyntaxError {
        self.buf.clear();
        if self.buf.push(b'\\').is_err() || self.buf.extend_from_slice(bytes).is_err() {
            return SyntaxError::from(Oom(self.buf.mem()));
        }
        self.buf_error(msg)
    }

    pub(super) fn escape_52(&mut self) -> Result<(), SyntaxError> {
        self.bump();
        let Some(c) = self.cur() else {
            return Ok(());
        };
        if let Some(e) = Self::simple_escape(c) {
            self.save(e)?;
            self.bump();
            return Ok(());
        }
        match c {
            b'x' => {
                let (mut seen, mut n) = ([b'x', 0, 0], 1);
                let mut v = 0;
                for _ in 0..2 {
                    self.bump();
                    let d = self.cur();
                    if let Some(d) = d {
                        seen[n] = d;
                        n += 1;
                    }
                    let Some(h) = d.and_then(hex_digit) else {
                        return Err(self.esc_error_52(&seen[..n], "hexadecimal digit expected"));
                    };
                    v = v * 16 + h;
                }
                self.bump();
                self.save(v as u8)?;
            }
            b'\n' | b'\r' => {
                self.newline();
                self.save(b'\n')?;
            }
            b'\\' | b'"' | b'\'' => {
                self.save(c)?;
                self.bump();
            }
            b'z' => {
                self.bump();
                self.skip_spaces();
            }
            b'0'..=b'9' => {
                let (mut seen, mut n) = ([0u8; 3], 0);
                let v = self.dec_digits(|_, d| {
                    seen[n] = d;
                    n += 1;
                    Ok(())
                })?;
                if v > 255 {
                    return Err(self.esc_error_52(&seen[..n], "decimal escape too large"));
                }
                self.save(v as u8)?;
            }
            _ => return Err(self.esc_error_52(&[c], "invalid escape sequence")),
        }
        Ok(())
    }

    /// `\z`: skip whitespace, counting lines.
    pub(super) fn skip_spaces(&mut self) {
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
    pub(super) fn esc_check(&mut self, ok: bool, msg: &str) -> Result<(), SyntaxError> {
        if ok {
            return Ok(());
        }
        if self.cur().is_some() {
            self.save_next()?;
        }
        Err(self.buf_error(msg))
    }

    /// 5.3+ `gethexa`: save the byte before, then demand a hex digit.
    pub(super) fn get_hexa(&mut self) -> Result<u32, SyntaxError> {
        self.save_next()?;
        let d = self.cur().and_then(hex_digit);
        self.esc_check(d.is_some(), "hexadecimal digit expected")?;
        Ok(d.expect("checked above"))
    }

    pub(super) fn drop_saved(&mut self, n: usize) {
        self.buf.truncate(self.buf.len() - n);
    }

    pub(super) fn escape_53(&mut self) -> Result<(), SyntaxError> {
        self.save_next()?;
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
                push_utf8(&mut self.buf, v)?;
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
                        lx.save(d)?;
                        n += 1;
                        Ok(())
                    })?;
                    self.esc_check(v <= 255, "decimal escape too large")?;
                    self.drop_saved(n);
                    v as u8
                }
            },
        };
        self.drop_saved(1);
        self.save(byte)?;
        Ok(())
    }

    /// 5.3+ `readutf8esc`, current at `u`; leaves the buffer as it found it
    /// minus the backslash. 5.3 caps the value at 0x10FFFF after adding each
    /// digit; 5.4 widened it to 2^31 and checks before shifting.
    pub(super) fn utf8_escape(&mut self) -> Result<u32, SyntaxError> {
        let mut saved = 4;
        self.save_next()?;
        self.esc_check(self.cur() == Some(b'{'), "missing '{'")?;
        let mut r = self.get_hexa()?;
        loop {
            self.save_next()?;
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
}

/// Extended UTF-8 (up to 6 bytes, values to 2^31-1), as luaO_utf8esc.
fn push_utf8(out: &mut LVec<u8>, mut x: u32) -> Result<(), Oom> {
    if x < 0x80 {
        return out.push(x as u8);
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
    out.reserve(n + 1)?;
    out.push(((!mfb << 1) | x) as u8)?;
    for &b in cont[..n].iter().rev() {
        out.push(b)?;
    }
    Ok(())
}
