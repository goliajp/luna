//! A `FILE` of the MSVC C library (the Universal CRT), which PUC built with
//! MSVC reads and writes through: its buffer, flags and the rules of
//! `_filbuf`, `_flsbuf`, `fread`, `fwrite`, `ungetc`, `fflush`, `fseek`,
//! `ftell` and `setvbuf`, over the text or binary mode of `msvc/lowio.rs`.
//! A `Vm` with [`Vm::set_crt_text_mode`] keeps one for each file it opens,
//! and the `luna` command on Windows one for each standard stream, so that
//! what a script sees (positions, failures, the bytes a file ends up with,
//! the order of output) is what `lua.exe` gives, quirks included.


mod lowio;
mod read;
pub(crate) mod scan;
mod write;

pub(crate) use lowio::{Os, drop_final_ctrl_z, translate_all};

const READ: u32 = 0x1;
const WRITE: u32 = 0x2;
const UPDATE: u32 = 0x4;
const EOF: u32 = 0x8;
const ERROR: u32 = 0x10;
const CTRLZ: u32 = 0x20;
const BUF_CRT: u32 = 0x40;
const BUF_USER: u32 = 0x80;
const BUF_SETVBUF: u32 = 0x100;
const BUF_STBUF: u32 = 0x200;
const BUF_NONE: u32 = 0x400;

/// `_INTERNAL_BUFSIZ`
pub(crate) const INTERNAL_BUFSIZ: usize = 4096;
/// `_SMALL_BUFSIZ`, what the first refill after a seek on a read-only
/// stream asks for
const SMALL_BUFSIZ: usize = 512;

const EINVAL: i32 = 22;
const EBADF: i32 = 9;

thread_local! {
    static ERRNO: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// The C library's `errno` as the emulated calls leave it.
pub(crate) fn errno() -> i32 {
    ERRNO.with(|e| e.get())
}

pub(crate) fn set_errno(v: i32) {
    ERRNO.with(|e| e.set(v));
}

/// What a standard stream is, for the library's temporary buffering.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Std {
    No,
    Out,
    Err,
}

/// One `FILE` and the low-level handle state behind it.
pub(crate) struct CrtFile {
    flags: u32,
    /// the buffer (`_base`); empty when the stream has none
    base: Vec<u8>,
    ptr: usize,
    cnt: i64,
    bufsiz: usize,
    pub(crate) io: lowio::Handle,
    std: Std,
    /// standard output on a console: written through a temporary buffer
    /// emptied at the end of each call
    tty: bool,
}

impl CrtFile {
    /// What `fopen` with `mode` makes, in text mode unless `text` is false.
    pub(crate) fn open(mode: &[u8], text: bool, pipe: bool) -> CrtFile {
        let flags = if mode.contains(&b'+') {
            UPDATE
        } else if mode.first() == Some(&b'r') {
            READ
        } else {
            WRITE
        };
        CrtFile {
            flags,
            base: Vec::new(),
            ptr: 0,
            cnt: 0,
            bufsiz: 0,
            io: lowio::Handle::new(text, pipe, false),
            std: Std::No,
            tty: false,
        }
    }

    /// `stdin`, `stdout` or `stderr`, in text mode.
    pub(crate) fn standard(which: Std, tty: bool) -> CrtFile {
        let flags = if which == Std::No { READ } else { WRITE };
        CrtFile {
            flags,
            base: Vec::new(),
            ptr: 0,
            cnt: 0,
            bufsiz: 0,
            io: lowio::Handle::new(true, !tty, tty),
            std: which,
            tty,
        }
    }

    fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    fn has_any_buffer(&self) -> bool {
        self.has(BUF_CRT | BUF_USER | BUF_NONE)
    }

    fn has_big_buffer(&self) -> bool {
        self.has(BUF_CRT | BUF_USER)
    }

    pub(crate) fn ferror(&self) -> bool {
        self.has(ERROR)
    }

    /// `clearerr`
    pub(crate) fn clearerr(&mut self) {
        self.flags &= !(EOF | ERROR);
        self.io.eof_flag = false;
    }

    fn allocate_buffer(&mut self) {
        self.base = vec![0; INTERNAL_BUFSIZ];
        self.flags |= BUF_CRT;
        self.bufsiz = INTERNAL_BUFSIZ;
        self.ptr = 0;
        self.cnt = 0;
    }

    fn reset_buffer(&mut self) {
        self.ptr = 0;
        self.cnt = 0;
    }

    fn flushable(&self) -> bool {
        self.flags & (READ | WRITE) == WRITE && self.has_big_buffer()
    }

    /// `__acrt_stdio_flush_nolock`; `false` when the write failed.
    fn flush(&mut self, os: &mut dyn Os) -> bool {
        if !self.flushable() {
            return true;
        }
        let n = self.ptr;
        self.reset_buffer();
        if n == 0 {
            return true;
        }
        let data = self.base[..n].to_vec();
        if self.io.write(os, &data) != n as i64 {
            self.flags |= ERROR;
            return false;
        }
        if self.has(UPDATE) {
            self.flags &= !WRITE;
        }
        true
    }

    /// `fflush`
    pub(crate) fn fflush(&mut self, os: &mut dyn Os) -> bool {
        self.flush(os)
    }

    /// `fclose`: what flushing gave.
    pub(crate) fn fclose(&mut self, os: &mut dyn Os) -> bool {
        let ok = self.flush(os);
        self.base = Vec::new();
        self.flags = 0;
        ok
    }

    /// `fseek` (`whence`: 0 set, 1 cur, 2 end); `false` with `errno` set
    /// on failure.
    pub(crate) fn fseek(&mut self, os: &mut dyn Os, offset: i64, whence: u8) -> bool {
        self.flags &= !EOF;
        if self.fast_seek(os, offset, whence) {
            return true;
        }
        let (mut offset, mut whence) = (offset, whence);
        if whence == 1 {
            offset = offset.wrapping_add(self.ftell(os));
            whence = 0;
        }
        self.flush(os);
        self.reset_buffer();
        if self.has(UPDATE) {
            self.flags &= !(WRITE | READ);
        } else if self.has(READ) && self.has(BUF_CRT) && !self.has(BUF_SETVBUF) {
            self.bufsiz = SMALL_BUFSIZ;
        }
        self.io.lseek(os, offset, whence) >= 0
    }

    /// `setvbuf(f, NULL, mode, size)` (`mode`: 0 full, 1 line, 2 none);
    /// `false` when it fails.
    pub(crate) fn setvbuf(&mut self, os: &mut dyn Os, mode: u8, size: usize) -> bool {
        self.flush(os);
        self.base = Vec::new();
        self.flags &= !(BUF_CRT | BUF_USER | BUF_NONE | BUF_SETVBUF | BUF_STBUF | CTRLZ);
        if mode == 2 {
            self.flags |= BUF_NONE;
            self.base = vec![0; 2];
            self.bufsiz = 2;
        } else {
            let usable = size & !1;
            self.flags |= BUF_CRT | BUF_SETVBUF;
            self.base = vec![0; usable];
            self.bufsiz = usable;
        }
        self.ptr = 0;
        self.cnt = 0;
        true
    }
}

fn count_lf(bytes: &[u8]) -> i64 {
    bytes.iter().filter(|&&b| b == b'\n').count() as i64
}

#[cfg(test)]
mod tests;
