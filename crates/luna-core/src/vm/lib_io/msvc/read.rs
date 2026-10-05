//! The reading half of a `FILE`: refills, `getc`, `ungetc`, `fread`, and
//! `ftell` with the positions it reports from the buffer.

use std::io::SeekFrom;

use super::*;

impl CrtFile {
    /// `_filbuf`: refill the buffer and take its first byte.
    pub(super) fn refill_and_read(&mut self, os: &mut dyn Os) -> Option<u8> {
        if self.has(WRITE) {
            self.flags |= ERROR;
            return None;
        }
        self.flags |= READ;
        if !self.has_any_buffer() {
            self.allocate_buffer();
        }
        self.ptr = 0;
        let size = self.bufsiz;
        let got = self.io.read(os, &mut self.base[..size]);
        if got <= 0 {
            self.flags |= if got == 0 { EOF } else { ERROR };
            self.cnt = 0;
            return None;
        }
        self.cnt = got;
        if !self.has(WRITE | UPDATE) && self.io.text && self.io.eof_flag {
            self.flags |= CTRLZ;
        }
        if self.bufsiz == SMALL_BUFSIZ && self.has(BUF_CRT) && !self.has(BUF_SETVBUF) {
            self.bufsiz = INTERNAL_BUFSIZ;
        }
        self.cnt -= 1;
        self.ptr = 1;
        Some(self.base[0])
    }

    /// `getc`
    pub(crate) fn getc(&mut self, os: &mut dyn Os) -> Option<u8> {
        self.cnt -= 1;
        if self.cnt >= 0 {
            let c = self.base[self.ptr];
            self.ptr += 1;
            return Some(c);
        }
        self.refill_and_read(os)
    }

    /// `ungetc`; `false` when the library refuses it (and the byte is lost).
    pub(crate) fn ungetc(&mut self, c: u8) -> bool {
        let reading = self.has(READ);
        let rw_writing = self.has(UPDATE) && self.has(WRITE);
        if !reading && !rw_writing {
            return false;
        }
        if self.base.is_empty() {
            self.allocate_buffer();
        }
        if self.ptr == 0 {
            if self.cnt != 0 {
                return false;
            }
            self.ptr += 1;
        }
        self.ptr -= 1;
        self.base[self.ptr] = c;
        self.cnt += 1;
        self.flags &= !EOF;
        self.flags |= READ;
        true
    }

    /// `fread` of up to `n` bytes.
    pub(crate) fn fread(&mut self, os: &mut dyn Os, n: usize) -> Vec<u8> {
        let mut sbs = if self.has_any_buffer() {
            self.bufsiz
        } else {
            INTERNAL_BUFSIZ
        };
        let mut out = Vec::new();
        while out.len() < n {
            let remaining = n - out.len();
            if self.has_any_buffer() && self.cnt != 0 {
                if self.cnt < 0 {
                    self.flags |= ERROR;
                    break;
                }
                let take = remaining.min(self.cnt as usize);
                out.extend_from_slice(&self.base[self.ptr..self.ptr + take]);
                self.cnt -= take as i64;
                self.ptr += take;
            } else if remaining >= sbs {
                let want = if sbs != 0 {
                    remaining - remaining % sbs
                } else {
                    remaining
                };
                self.reset_buffer();
                let mut chunk = vec![0; want];
                let got = self.io.read(os, &mut chunk);
                if got == 0 {
                    self.flags |= EOF;
                    break;
                }
                if got < 0 {
                    self.flags |= ERROR;
                    break;
                }
                out.extend_from_slice(&chunk[..got as usize]);
            } else {
                match self.refill_and_read(os) {
                    Some(c) => out.push(c),
                    None => break,
                }
                sbs = self.bufsiz;
            }
        }
        out
    }

    /// `ftell`; -1 with `errno` set on failure.
    pub(crate) fn ftell(&mut self, os: &mut dyn Os) -> i64 {
        if self.cnt < 0 {
            self.cnt = 0;
        }
        let lowio = match os.seek(SeekFrom::Current(0)) {
            Ok(p) => p as i64,
            Err(_) => {
                set_errno(EINVAL);
                return -1;
            }
        };
        if !self.has_big_buffer() {
            return lowio - self.cnt;
        }
        let mut offset = self.ptr as i64;
        if self.has(WRITE | READ) {
            if self.io.text {
                offset += count_lf(&self.base[..self.ptr]);
            }
        } else if !self.has(UPDATE) {
            set_errno(EINVAL);
            return -1;
        }
        if lowio == 0 {
            return offset;
        }
        if self.has(READ) {
            return self.ftell_read(os, lowio, offset);
        }
        lowio + offset
    }

    pub(super) fn ftell_read(&mut self, os: &mut dyn Os, lowio: i64, offset: i64) -> i64 {
        if self.cnt == 0 {
            return lowio;
        }
        let mut bytes_read = self.cnt + self.ptr as i64;
        if !self.io.text {
            return lowio - bytes_read + offset;
        }
        if os.seek(SeekFrom::End(0)).map(|p| p as i64).ok() == Some(lowio) {
            bytes_read += count_lf(&self.base[..bytes_read as usize]);
            if self.has(CTRLZ) {
                bytes_read += 1;
            }
        } else {
            if os.seek(SeekFrom::Start(lowio as u64)).is_err() {
                return -1;
            }
            bytes_read = if bytes_read <= SMALL_BUFSIZ as i64
                && self.has(BUF_CRT)
                && !self.has(BUF_SETVBUF)
            {
                SMALL_BUFSIZ as i64
            } else {
                self.bufsiz as i64
            };
            if self.io.crlf {
                bytes_read += 1;
            }
        }
        lowio - bytes_read + offset
    }

    /// A seek within the buffer of a binary stream opened for reading only.
    pub(super) fn fast_seek(&mut self, os: &mut dyn Os, offset: i64, whence: u8) -> bool {
        if whence == 2 || !self.has_any_buffer() || self.has(WRITE | UPDATE) || self.cnt <= 0 {
            return false;
        }
        if self.io.text {
            return false;
        }
        let mut offset = offset;
        if whence == 0 {
            let Ok(lowio) = os.seek(SeekFrom::Current(0)) else {
                return false;
            };
            let Some(o) = offset.checked_sub(lowio as i64 - self.cnt) else {
                return false;
            };
            offset = o;
        }
        if -(self.ptr as i64) <= offset && offset <= self.cnt {
            self.ptr = (self.ptr as i64 + offset) as usize;
            self.cnt -= offset;
            return true;
        }
        false
    }
}
