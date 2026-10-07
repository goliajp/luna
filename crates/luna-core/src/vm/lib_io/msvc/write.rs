//! The writing half of a `FILE`: `fwrite`, `_flsbuf`, and the temporary
//! buffer standard streams get for one call.

use std::io::SeekFrom;

use super::*;

impl CrtFile {
    /// Whether the stream's file is at its end, for a write after a read.
    pub(super) fn at_end_of_file(&mut self, os: &mut dyn Os) -> bool {
        if self.has(EOF) {
            return true;
        }
        if self.has_big_buffer() && self.ptr == 0 {
            return false;
        }
        match (os.seek(SeekFrom::Current(0)), os.len()) {
            (Ok(p), Ok(l)) => p == l,
            _ => false,
        }
    }

    /// `_flsbuf`: write out the buffer and put `c` in it.
    pub(super) fn flush_and_write(&mut self, os: &mut dyn Os, c: u8) -> bool {
        if !self.has(WRITE | UPDATE) {
            set_errno(EBADF);
            self.flags |= ERROR;
            return false;
        }
        if self.has(READ) {
            let switch = self.at_end_of_file(os);
            self.cnt = 0;
            if switch {
                self.ptr = 0;
                self.flags &= !READ;
            } else {
                self.flags |= ERROR;
                return false;
            }
        }
        self.flags |= WRITE;
        self.flags &= !EOF;
        self.cnt = 0;
        if !self.has_any_buffer() && !self.temporary() {
            self.allocate_buffer();
        }
        if self.has_big_buffer() {
            let n = self.ptr;
            self.ptr = 1;
            self.cnt = self.bufsiz as i64 - 1;
            let ok = if n > 0 {
                let data = self.base[..n].to_vec();
                self.io.write(os, &data) == n as i64
            } else {
                if self.io.append {
                    let _ = os.seek(SeekFrom::End(0));
                }
                true
            };
            self.base[0] = c;
            if !ok {
                self.flags |= ERROR;
            }
            ok
        } else {
            let ok = self.io.write(os, &[c]) == 1;
            if !ok {
                self.flags |= ERROR;
            }
            ok
        }
    }

    pub(super) fn temporary(&self) -> bool {
        self.std == Std::Err || (self.std == Std::Out && self.tty)
    }

    /// Give a standard stream without a buffer of its own a temporary one
    /// for the length of one call; `true` when it did.
    pub(super) fn begin_temporary(&mut self) -> bool {
        if !self.temporary() || self.has_any_buffer() {
            return false;
        }
        self.flags |= WRITE | BUF_USER | BUF_STBUF;
        self.base = vec![0; INTERNAL_BUFSIZ];
        self.ptr = 0;
        self.cnt = INTERNAL_BUFSIZ as i64;
        self.bufsiz = INTERNAL_BUFSIZ;
        true
    }

    pub(super) fn end_temporary(&mut self, os: &mut dyn Os, began: bool) {
        if began && self.has(BUF_STBUF) {
            self.flush(os);
            self.flags &= !(BUF_USER | BUF_STBUF);
            self.bufsiz = 0;
            self.base = Vec::new();
            self.ptr = 0;
        }
    }

    /// `fwrite`; the number of bytes it took.
    pub(crate) fn fwrite(&mut self, os: &mut dyn Os, data: &[u8]) -> usize {
        let began = self.begin_temporary();
        let n = self.fwrite_nolock(os, data);
        self.end_temporary(os, began);
        n
    }

    pub(super) fn fwrite_nolock(&mut self, os: &mut dyn Os, data: &[u8]) -> usize {
        let mut sbs = if self.has_any_buffer() {
            self.bufsiz
        } else {
            INTERNAL_BUFSIZ
        };
        let mut done = 0;
        while done < data.len() {
            let remaining = data.len() - done;
            if self.has_big_buffer() && self.cnt != 0 {
                if self.cnt < 0 || self.has(READ) {
                    if self.cnt < 0 {
                        self.flags |= ERROR;
                    }
                    return done;
                }
                let n = remaining.min(self.cnt as usize);
                self.base[self.ptr..self.ptr + n].copy_from_slice(&data[done..done + n]);
                done += n;
                self.cnt -= n as i64;
                self.ptr += n;
            } else if remaining >= sbs {
                if self.has_big_buffer() && !self.flush(os) {
                    return done;
                }
                let want = if sbs > 0 {
                    remaining - remaining % sbs
                } else {
                    remaining
                };
                let wrote = self.io.write(os, &data[done..done + want]);
                if wrote < 0 {
                    self.flags |= ERROR;
                    return done;
                }
                let wrote = (wrote as usize).min(want);
                done += wrote;
                if wrote < want {
                    self.flags |= ERROR;
                    return done;
                }
            } else {
                if !self.flush_and_write(os, data[done]) {
                    return done;
                }
                done += 1;
                sbs = if self.bufsiz > 0 { self.bufsiz } else { 1 };
            }
        }
        data.len()
    }
}
