//! The Unicode text modes of the MSVC C library's low-level I/O, which a
//! `ccs=` in an `fopen` mode selects (PUC 5.1 passes the mode on as it is):
//! in UTF-16LE mode the stream's bytes are UTF-16 code units, written as
//! they are; in UTF-8 mode the file holds UTF-8 and the stream sees it
//! converted to UTF-16. `\n` is written as `\r\n` and `\r\n` read as `\n`,
//! a code unit at a time. A count of bytes that is not a whole number of
//! code units is an invalid argument, which ends the process.

use std::io::SeekFrom;

use super::lowio::{Handle, Os};
use super::{EINVAL, set_errno};

const CR: u16 = b'\r' as u16;
const LF: u16 = b'\n' as u16;
const CTRL_Z: u16 = 0x1a;
const EILSEQ: i32 = 42;

/// The text mode of a handle (`__crt_lowio_text_mode`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextMode {
    Ansi,
    Utf8,
    Utf16le,
}

/// What `_utf8_no_of_trailbytes` gives for a lead byte; 0 for any other.
pub(super) fn trail_bytes(b: u8) -> usize {
    match b {
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        _ => 0,
    }
}

/// `_utf8_is_independent`: an ASCII byte.
fn is_independent(b: u8) -> bool {
    b < 0x80
}

/// `_utf8_is_leadbyte`
fn is_lead(b: u8) -> bool {
    trail_bytes(b) != 0
}

/// The newlines among the code units of `bytes`, in bytes, as `ftell`
/// counts them. The library walks code units until it lands exactly on the
/// end, so an odd count runs it off the buffer: the process ends with an
/// access violation.
pub(crate) fn newline_bytes(bytes: &[u8], mode: TextMode) -> i64 {
    if mode == TextMode::Ansi {
        return bytes.iter().filter(|&&b| b == b'\n').count() as i64;
    }
    if bytes.len() % 2 != 0 {
        access_violation();
    }
    let n = bytes
        .chunks_exact(2)
        .filter(|u| u16::from_le_bytes([u[0], u[1]]) == LF)
        .count();
    2 * n as i64
}

/// The end of a process that read memory it may not.
pub(crate) fn access_violation() -> ! {
    std::process::exit(0xC000_0005_u32 as i32)
}

fn units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|u| u16::from_le_bytes([u[0], u[1]]))
        .collect()
}

fn put_units(dst: &mut [u8], units: &[u16]) {
    for (d, u) in dst.chunks_exact_mut(2).zip(units) {
        d.copy_from_slice(&u.to_le_bytes());
    }
}

impl Handle {
    /// `_read` in UTF-16LE mode.
    pub(super) fn read_utf16(&mut self, os: &mut dyn Os, dst: &mut [u8]) -> i64 {
        if dst.len() % 2 != 0 {
            super::super::crt::invalid_parameter();
        }
        let have = match os.read(dst) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
            Err(_) => return -1,
        };
        if !self.text || have == 0 {
            return have as i64;
        }
        let mut buf = units(&dst[..have]);
        let n = self.translate_units(os, &mut buf);
        put_units(dst, &buf[..n]);
        2 * n as i64
    }

    /// `translate_text_mode_nolock` on code units, in place: how many are
    /// left.
    fn translate_units(&mut self, os: &mut dyn Os, buf: &mut [u16]) -> usize {
        self.crlf = buf.first() == Some(&LF);
        let count = buf.len();
        let (mut src, mut out) = (0, 0);
        while src < count {
            let c = buf[src];
            if c == CTRL_Z {
                if self.is_dev() {
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
                buf[out] = if buf[src + 1] == LF {
                    src += 2;
                    LF
                } else {
                    src += 1;
                    CR
                };
                out += 1;
                continue;
            }
            src += 1;
            let mut peek = [0u8; 2];
            let peek = match os.read(&mut peek) {
                Ok(n) if n > 0 => u16::from_le_bytes(peek),
                _ => {
                    buf[out] = CR;
                    out += 1;
                    continue;
                }
            };
            if peek == LF && out == 0 {
                buf[out] = LF;
                out += 1;
            } else {
                let _ = os.seek(SeekFrom::Current(-2));
                if peek != LF {
                    buf[out] = CR;
                    out += 1;
                }
            }
        }
        out
    }

    /// `_read` in UTF-8 mode: the file's bytes, `\r\n` turned to `\n`, up
    /// to the last whole character, as UTF-16 (`MultiByteToWideChar`, which
    /// puts U+FFFD for what is not UTF-8).
    pub(super) fn read_utf8(&mut self, os: &mut dyn Os, dst: &mut [u8]) -> i64 {
        if dst.len() % 2 != 0 {
            super::super::crt::invalid_parameter();
        }
        let mut raw = vec![0u8; (dst.len() / 2).max(4)];
        self.startpos = os.seek(SeekFrom::Current(0)).map_or(-1, |p| p as i64);
        let have = match os.read(&mut raw) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
            Err(_) => return -1,
        };
        if have == 0 {
            return 0;
        }
        let n = self.translate(os, &mut raw, have);
        if n == 0 {
            return 0;
        }
        let mut end = n;
        if !is_independent(raw[n - 1]) {
            // back to the lead byte of the last character
            let mut i = n - 1;
            let mut counter = 1;
            while !is_lead(raw[i]) && counter <= 4 && i > 0 {
                i -= 1;
                counter += 1;
            }
            let trail = trail_bytes(raw[i]);
            if trail == 0 {
                set_errno(EILSEQ);
                return -1;
            }
            if trail + 1 == counter {
                end = i + counter;
            } else {
                // read the character again next time
                let _ = os.seek(SeekFrom::Current(-(counter as i64)));
                end = i;
            }
        }
        let wide: Vec<u16> = String::from_utf8_lossy(&raw[..end])
            .encode_utf16()
            .collect();
        if wide.is_empty() || wide.len() > dst.len() / 2 {
            set_errno(EINVAL);
            return -1;
        }
        self.utf8_translations = wide.len() != end;
        put_units(dst, &wide);
        2 * wide.len() as i64
    }

    /// `_write` in UTF-16LE mode: each `\n` code unit after a `\r` one.
    pub(super) fn write_utf16(&mut self, os: &mut dyn Os, data: &[u8]) -> i64 {
        let mut out = Vec::with_capacity(data.len() + data.len() / 8);
        for u in units(data) {
            if u == LF {
                out.extend_from_slice(&CR.to_le_bytes());
            }
            out.extend_from_slice(&u.to_le_bytes());
        }
        match os.write_all(&out) {
            Ok(()) => data.len() as i64,
            Err(_) => -1,
        }
    }

    /// `_write` in UTF-8 mode: the code units with `\r` before each `\n`,
    /// converted to UTF-8 in pieces of up to 852 units, each on its own
    /// (`WideCharToMultiByte`, which puts U+FFFD for a lone surrogate, also
    /// for half of a pair a piece boundary splits).
    pub(super) fn write_utf8(&mut self, os: &mut dyn Os, data: &[u8]) -> i64 {
        const PIECE: usize = 5 * 1024 / 6 - 1;
        let src = units(data);
        let mut i = 0;
        while i < src.len() {
            let mut piece = Vec::with_capacity(PIECE + 1);
            while piece.len() < PIECE && i < src.len() {
                if src[i] == LF {
                    piece.push(CR);
                }
                piece.push(src[i]);
                i += 1;
            }
            if os
                .write_all(String::from_utf16_lossy(&piece).as_bytes())
                .is_err()
            {
                return -1;
            }
        }
        data.len() as i64
    }
}
