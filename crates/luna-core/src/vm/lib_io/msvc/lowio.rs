//! The MSVC C library's low-level I/O on one handle (`_read`, `_write`,
//! `_lseek`): in text mode `\r\n` reads as `\n`, a Ctrl+Z ends the input,
//! and `\n` is written as `\r\n`.

use std::io::SeekFrom;

use super::{EINVAL, set_errno};

const CR: u8 = b'\r';
const LF: u8 = b'\n';
const CTRL_Z: u8 = 0x1a;

/// The OS file under a stream.
pub(crate) trait Os {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64>;
    fn len(&mut self) -> std::io::Result<u64>;
}

impl Os for std::fs::File {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(self, buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(self, buf)
    }
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(self, from)
    }
    fn len(&mut self) -> std::io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

/// What the library keeps for one handle (`_osfile` and friends).
pub(crate) struct Handle {
    /// `FTEXT`
    pub(crate) text: bool,
    /// `FCRLF`: the last read began with a `\n`
    pub(crate) crlf: bool,
    /// `FEOFLAG`: a Ctrl+Z was read; reads give nothing until a seek
    pub(crate) eof_flag: bool,
    /// `FPIPE`: no seeking back over a byte read past a final `\r`
    pipe: bool,
    /// `FDEV`: a console, where a Ctrl+Z is passed on and ends only the
    /// read it is in
    dev: bool,
    /// `FAPPEND`
    pub(crate) append: bool,
    /// the pipe lookahead byte
    lookahead: Option<u8>,
}

impl Handle {
    pub(crate) fn new(text: bool, pipe: bool, dev: bool) -> Handle {
        Handle {
            text,
            crlf: false,
            eof_flag: false,
            pipe,
            dev,
            append: false,
            lookahead: None,
        }
    }

    /// `_read` into `dst`: the number of bytes it gives, 0 at end of file,
    /// -1 on an error.
    pub(crate) fn read(&mut self, os: &mut dyn Os, dst: &mut [u8]) -> i64 {
        if dst.is_empty() || self.eof_flag {
            return 0;
        }
        let mut have = 0;
        if (self.pipe || self.dev)
            && let Some(b) = self.lookahead.take()
        {
            dst[0] = b;
            have = 1;
        }
        match os.read(&mut dst[have..]) {
            Ok(n) => have += n,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(_) => return -1,
        }
        if !self.text || have == 0 {
            return have as i64;
        }
        self.translate(os, dst, have) as i64
    }

    /// `translate_text_mode_nolock` over `buf[..count]`, in place.
    fn translate(&mut self, os: &mut dyn Os, buf: &mut [u8], count: usize) -> usize {
        self.crlf = buf[0] == LF;
        let (mut src, mut out) = (0, 0);
        while src < count {
            let c = buf[src];
            if c == CTRL_Z {
                if self.dev {
                    buf[out] = c;
                    out += 1;
                } else {
                    self.eof_flag = true;
                }
                break;
            }
            if c != CR {
                buf[out] = c;
                out += 1;
                src += 1;
                continue;
            }
            if src + 1 < count {
                if buf[src + 1] == LF {
                    buf[out] = LF;
                    src += 2;
                } else {
                    buf[out] = CR;
                    src += 1;
                }
                out += 1;
                continue;
            }
            // a `\r` that ends the read: look at the next byte
            src += 1;
            let mut peek = [0u8; 1];
            if !matches!(os.read(&mut peek), Ok(1)) {
                buf[out] = CR;
                out += 1;
                continue;
            }
            if self.pipe || self.dev {
                if peek[0] == LF {
                    buf[out] = LF;
                } else {
                    buf[out] = CR;
                    self.lookahead = Some(peek[0]);
                }
                out += 1;
            } else if peek[0] == LF && out == 0 {
                buf[out] = LF;
                out += 1;
            } else {
                let _ = os.seek(SeekFrom::Current(-1));
                if peek[0] != LF {
                    buf[out] = CR;
                    out += 1;
                }
            }
        }
        out
    }

    /// `_write` of `data`: the number of its bytes written, -1 on failure.
    pub(crate) fn write(&mut self, os: &mut dyn Os, data: &[u8]) -> i64 {
        let r = if self.text && data.contains(&LF) {
            let mut out = Vec::with_capacity(data.len() + data.len() / 8);
            for &b in data {
                if b == LF {
                    out.push(CR);
                }
                out.push(b);
            }
            os.write_all(&out)
        } else {
            os.write_all(data)
        };
        match r {
            Ok(()) => data.len() as i64,
            Err(_) => -1,
        }
    }

    /// `_lseeki64`: the new position, or -1 with `errno` set.
    pub(crate) fn lseek(&mut self, os: &mut dyn Os, offset: i64, whence: u8) -> i64 {
        let from = match whence {
            0 if offset < 0 => {
                set_errno(EINVAL);
                return -1;
            }
            0 => SeekFrom::Start(offset as u64),
            1 => SeekFrom::Current(offset),
            _ => SeekFrom::End(offset),
        };
        match os.seek(from) {
            Ok(p) => {
                self.eof_flag = false;
                p as i64
            }
            Err(_) => {
                set_errno(EINVAL);
                -1
            }
        }
    }
}

/// A whole source file as reading it in text mode gives it: `\r\n` becomes
/// `\n` and a Ctrl+Z ends it.
pub(crate) fn translate_all(raw: &[u8]) -> Vec<u8> {
    let end = raw.iter().position(|&b| b == CTRL_Z).unwrap_or(raw.len());
    let raw = &raw[..end];
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == CR && raw.get(i + 1) == Some(&LF) {
            i += 1;
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// Opening a text mode file for reading and writing drops a Ctrl+Z that
/// ends it, so that appending works.
pub(crate) fn drop_final_ctrl_z(f: &mut std::fs::File) -> std::io::Result<()> {
    use std::io::{Read, Seek};
    let len = Seek::seek(f, SeekFrom::End(0))?;
    if len > 0 {
        Seek::seek(f, SeekFrom::Start(len - 1))?;
        let mut last = [0u8; 1];
        if Read::read(f, &mut last)? == 1 && last[0] == CTRL_Z {
            f.set_len(len - 1)?;
        }
    }
    Ok(())
}
