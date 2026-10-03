//! The process's standard output, as C stdio buffers it.
//!
//! By default luna writes standard output through Rust's `std::io::stdout`,
//! which flushes at every newline, so its output stays in order with a
//! host's own `println!`. A program that stands in for PUC's `lua`
//! interpreter calls [`use_c_stdout`]: standard output then behaves as C
//! stdio's `stdout` does under glibc, so output written to it and messages
//! written to the unbuffered standard error reach a shared pipe or file in
//! the same order as with PUC. Such a program must call [`flush_stdout`]
//! before it exits, as C's `exit` does.
//!
//! The buffer follows glibc: line buffered on a terminal, fully buffered
//! otherwise, `min(st_blksize, BUFSIZ)` bytes, and the same rules for when
//! a write goes out (`_IO_new_file_xsputn`, `_IO_new_file_overflow`,
//! `_IO_default_xsputn`). Other C libraries flush at the same points (a
//! newline on a terminal, `fflush`, a full buffer) but may split a write
//! longer than the buffer differently.

use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static C_MODE: AtomicBool = AtomicBool::new(false);
static STDOUT: Mutex<CFile> = Mutex::new(CFile::new());

/// From now on, buffer standard output as C stdio does (see the module
/// documentation). Process-wide, like C's `stdout`.
pub fn use_c_stdout() {
    C_MODE.store(true, Ordering::Relaxed);
}

/// Whether [`use_c_stdout`] is in effect.
pub(crate) fn c_mode() -> bool {
    C_MODE.load(Ordering::Relaxed)
}

/// `fwrite(bytes, 1, n, stdout)`. Errors are dropped, as `print` drops
/// them; `io.write` reports them.
pub fn write_stdout(bytes: &[u8]) {
    let _ = try_write_stdout(bytes);
}

/// `fflush(stdout)`.
pub fn flush_stdout() -> std::io::Result<()> {
    if c_mode() {
        lock().flush_buf()
    } else {
        std::io::stdout().flush()
    }
}

/// `fwrite` of a whole `print` line followed by `fflush(stdout)` (5.2's
/// `lua_writeline` on), under one lock.
pub(crate) fn write_line_flushed(bytes: &[u8]) {
    if c_mode() {
        let mut f = lock();
        if f.allocated && f.buf.is_empty() && bytes.len() <= f.cap {
            // what copying it into the empty buffer and flushing writes
            let _ = f.raw_write(bytes);
        } else {
            let _ = f.xsputn(bytes).and_then(|()| f.flush_buf());
        }
    } else {
        let _ = std::io::stdout().write_all(bytes);
    }
}

pub(crate) fn try_write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    if c_mode() {
        lock().xsputn(bytes)
    } else {
        std::io::stdout().write_all(bytes)
    }
}

/// `setvbuf(stdout, NULL, mode, size)`: `0` full, `1` line, `2` none.
pub(crate) fn setvbuf_stdout(mode: u8) {
    if c_mode() {
        lock().setvbuf(mode);
    }
}

/// glibc refilling a line buffered or unbuffered input stream (stdin on a
/// terminal) first writes out `stdout` if that is line buffered.
pub(crate) fn before_stdin_read() {
    if c_mode() && std::io::stdin().is_terminal() {
        let mut f = lock();
        if f.allocated && f.line {
            let _ = f.flush_buf();
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, CFile> {
    // a panic while the buffer was locked leaves it consistent: every
    // method either completes a step or returns before changing it
    STDOUT.lock().unwrap_or_else(|p| p.into_inner())
}

/// glibc's `FILE` for `stdout`, reduced to what writing needs. `write_end`
/// is `_IO_write_end - _IO_buf_base`: 0 while line buffered or unbuffered,
/// so every byte goes through `overflow`, `cap` while fully buffered.
struct CFile {
    /// the descriptor when it is not a terminal, written directly, without
    /// the line handling of Rust's stdout
    fd: Option<std::mem::ManuallyDrop<std::fs::File>>,
    buf: Vec<u8>,
    cap: usize,
    write_end: usize,
    allocated: bool,
    line: bool,
    unbuffered: bool,
}

impl CFile {
    const fn new() -> CFile {
        CFile {
            fd: None,
            buf: Vec::new(),
            cap: 0,
            write_end: 0,
            allocated: false,
            line: false,
            unbuffered: false,
        }
    }

    /// `_IO_file_doallocate`: the buffer size, and line buffering on a
    /// terminal.
    fn allocate(&mut self) {
        let out = std::io::stdout();
        self.cap = buffer_size(&out);
        if out.is_terminal() {
            self.line = true;
        } else {
            self.fd = dup(&out);
        }
        self.buf = Vec::with_capacity(self.cap);
        self.allocated = true;
    }

    fn reset_write_end(&mut self) {
        self.write_end = if self.line || self.unbuffered {
            0
        } else {
            self.cap
        };
    }

    /// `new_do_write`: `bytes` to the descriptor. Any write leaves the
    /// buffer empty and resets `write_end`.
    fn raw_write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let r = match &mut self.fd {
            Some(fd) => (&**fd).write_all(bytes),
            None => {
                let mut out = std::io::stdout().lock();
                out.write_all(bytes).and_then(|()| out.flush())
            }
        };
        self.reset_write_end();
        r
    }

    /// `_IO_do_write` of the whole buffer. C stdio drops what it failed to
    /// write.
    fn flush_buf(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let buf = std::mem::take(&mut self.buf);
        let r = self.raw_write(&buf);
        self.buf = buf;
        self.buf.clear();
        r
    }

    /// `_IO_new_file_overflow`: set up for writing; then `None` flushes,
    /// `Some(c)` stores `c`, flushing a full buffer first and, line
    /// buffered, after a newline.
    fn overflow(&mut self, c: Option<u8>) -> std::io::Result<()> {
        if !self.allocated {
            self.allocate();
            self.reset_write_end();
        }
        let Some(c) = c else {
            return self.flush_buf();
        };
        if self.buf.len() >= self.cap {
            self.flush_buf()?;
        }
        self.buf.push(c);
        if self.unbuffered || (self.line && c == b'\n') {
            self.flush_buf()?;
        }
        Ok(())
    }

    /// `_IO_new_file_xsputn`.
    fn xsputn(&mut self, s: &[u8]) -> std::io::Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        let mut must_flush = false;
        let mut count = if self.line && self.allocated {
            let room = self.cap - self.buf.len();
            if room >= s.len()
                && let Some(nl) = s.iter().rposition(|&b| b == b'\n')
            {
                must_flush = true;
                nl + 1
            } else {
                room
            }
        } else {
            self.write_end.saturating_sub(self.buf.len())
        };
        count = count.min(s.len());
        self.buf.extend_from_slice(&s[..count]);
        let rest = &s[count..];
        if rest.is_empty() && !must_flush {
            return Ok(());
        }
        self.overflow(None)?;
        // whole blocks go straight to the descriptor
        let block = self.cap;
        let direct = rest.len() - if block >= 128 { rest.len() % block } else { 0 };
        if direct > 0 {
            self.raw_write(&rest[..direct])?;
        }
        self.default_xsputn(&rest[direct..])
    }

    /// `_IO_default_xsputn`: copy what fits below `write_end`, the rest a
    /// byte at a time through `overflow`.
    fn default_xsputn(&mut self, mut s: &[u8]) -> std::io::Result<()> {
        loop {
            if self.write_end > self.buf.len() {
                let n = (self.write_end - self.buf.len()).min(s.len());
                self.buf.extend_from_slice(&s[..n]);
                s = &s[n..];
            }
            let Some((&c, rest)) = s.split_first() else {
                return Ok(());
            };
            self.overflow(Some(c))?;
            s = rest;
        }
    }

    /// glibc `setvbuf` with a NULL buffer: full and line only change the
    /// mode (full allocates first, which on a terminal would pick line);
    /// none flushes and leaves a one-byte buffer.
    fn setvbuf(&mut self, mode: u8) {
        match mode {
            0 => {
                if !self.allocated {
                    self.allocate();
                }
                self.line = false;
                self.unbuffered = false;
            }
            1 => {
                self.unbuffered = false;
                self.line = true;
            }
            _ => {
                let _ = self.flush_buf();
                self.line = false;
                self.unbuffered = true;
                self.allocated = true;
                self.cap = 1;
                self.write_end = 0;
            }
        }
    }
}

/// Descriptor 1 itself, as C stdio writes it. Not a duplicate: Linux
/// serialises writes through an open file shared by two descriptors
/// (`f_pos_lock`), which costs every `print` to a file.
#[cfg(unix)]
fn dup(out: &std::io::Stdout) -> Option<std::mem::ManuallyDrop<std::fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: the `File` is never dropped (`ManuallyDrop`), so it never
    // closes descriptor 1, which the process owns; writing through it
    // while it is closed or reused fails or reaches what is there now, as
    // C stdio's writes to descriptor 1 do
    let f = unsafe { std::fs::File::from_raw_fd(out.as_raw_fd()) };
    Some(std::mem::ManuallyDrop::new(f))
}

#[cfg(windows)]
fn dup(out: &std::io::Stdout) -> Option<std::mem::ManuallyDrop<std::fs::File>> {
    use std::os::windows::io::AsHandle;
    let h = out.as_handle().try_clone_to_owned().ok()?;
    Some(std::mem::ManuallyDrop::new(std::fs::File::from(h)))
}

#[cfg(not(any(unix, windows)))]
fn dup(_out: &std::io::Stdout) -> Option<std::mem::ManuallyDrop<std::fs::File>> {
    None
}

/// `BUFSIZ`, and glibc's use of a smaller `st_blksize`.
#[cfg(unix)]
fn buffer_size(out: &std::io::Stdout) -> usize {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;
    const BUFSIZ: usize = 8192;
    let blksize = out
        .as_fd()
        .try_clone_to_owned()
        .and_then(|fd| std::fs::File::from(fd).metadata())
        .map_or(0, |m| m.blksize() as usize);
    if blksize > 0 && blksize < BUFSIZ {
        blksize
    } else {
        BUFSIZ
    }
}

/// The C runtime's default stream buffer outside Unix.
#[cfg(not(unix))]
fn buffer_size(_out: &std::io::Stdout) -> usize {
    4096
}
