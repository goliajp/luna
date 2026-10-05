//! Standard output and standard error as the MSVC C library has them, for
//! the `luna` command on Windows: a `FILE` each, in text mode. Output to a
//! pipe or file is buffered in 4096-byte blocks (`"line"` buffering is full
//! buffering there); on a console, and always for standard error, each call
//! is written out when it returns, unless `setvbuf` gave the stream a buffer.

use std::io::SeekFrom;
use std::sync::Mutex;

use crate::vm::lib_io::msvc::{CrtFile, Os, Std};

struct Stream {
    file: CrtFile,
    os: Option<std::fs::File>,
}

static OUT: Mutex<Option<Stream>> = Mutex::new(None);
static ERR: Mutex<Option<Stream>> = Mutex::new(None);

/// A duplicate of the stream's descriptor; none when the process has no
/// such stream, and then every write fails, as the library's do.
struct Raw<'a>(Option<&'a mut std::fs::File>);

fn closed() -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::BrokenPipe)
}

impl Os for Raw<'_> {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(self.0.as_mut().ok_or_else(closed)?, buf)
    }
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(self.0.as_mut().ok_or_else(closed)?, from)
    }
    fn len(&mut self) -> std::io::Result<u64> {
        Ok(self.0.as_mut().ok_or_else(closed)?.metadata()?.len())
    }
}

fn open(which: Std) -> Stream {
    use std::io::IsTerminal;
    let (tty, os) = match which {
        Std::Err => (std::io::stderr().is_terminal(), super::dup_stderr()),
        _ => (std::io::stdout().is_terminal(), super::dup_stdout()),
    };
    Stream {
        file: CrtFile::standard(which, tty),
        os,
    }
}

fn with<R>(which: Std, f: impl FnOnce(&mut CrtFile, &mut dyn Os) -> R) -> R {
    let lock = if which == Std::Err { &ERR } else { &OUT };
    let mut g = lock.lock().unwrap_or_else(|p| p.into_inner());
    let s = g.get_or_insert_with(|| open(which));
    f(&mut s.file, &mut Raw(s.os.as_mut()))
}

fn result(ok: bool) -> std::io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other("write to a standard stream failed"))
    }
}

/// `fwrite(bytes, 1, n, stream)`
pub(super) fn write(which: Std, bytes: &[u8]) -> std::io::Result<()> {
    result(with(which, |f, os| f.fwrite(os, bytes)) == bytes.len())
}

/// `fflush(stream)`
pub(super) fn flush(which: Std) -> std::io::Result<()> {
    result(with(which, |f, os| f.fflush(os)))
}

/// `setvbuf(stream, NULL, mode, size)`
pub(super) fn setvbuf(which: Std, mode: u8, size: usize) {
    with(which, |f, os| f.setvbuf(os, mode, size));
}
