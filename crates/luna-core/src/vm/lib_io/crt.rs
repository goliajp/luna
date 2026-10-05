//! The io library's files when the Vm opens them as PUC built with MSVC
//! does: each carries the C library's `FILE` (`Userdata::crt`), and the
//! stream functions go through it instead of luna's own buffers.

use super::msvc::{CrtFile, Os};
use super::*;

/// Standard input under a `FILE`.
struct StdinOs;

impl Os for StdinOs {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::io::stdin().read(buf)
    }
    fn write_all(&mut self, _buf: &[u8]) -> std::io::Result<()> {
        Err(posix_error(EBADF))
    }
    fn seek(&mut self, _from: SeekFrom) -> std::io::Result<u64> {
        Err(posix_error(ESPIPE))
    }
    fn len(&mut self) -> std::io::Result<u64> {
        Err(posix_error(ESPIPE))
    }
}

/// Run `f` on the stream's `FILE` and its OS file.
pub(super) fn with<R>(u: Gc<Userdata>, f: impl FnOnce(&mut CrtFile, &mut dyn Os) -> R) -> R {
    // SAFETY: `u` is a file handle the caller holds; the borrow covers the one call, which runs no Lua code and takes no other reference into `u`
    let m = unsafe { u.as_mut() };
    let crt = m.crt.as_deref_mut().expect("a CRT stream");
    match &mut m.payload {
        UserdataPayload::File(FileHandle::File(file)) => f(crt, file),
        UserdataPayload::File(FileHandle::Stdin) => f(crt, &mut StdinOs),
        _ => unreachable!("only files and standard input carry a FILE"),
    }
}

/// Give `u` the `FILE` that `fopen(name, mode)` makes.
pub(super) fn attach(u: Gc<Userdata>, f: CrtFile) {
    // SAFETY: `u` was just made by the caller and is held only by it; the borrow covers one field store
    unsafe { u.as_mut() }.crt = Some(Box::new(f));
}

/// The error a failed C call leaves behind: `errno`.
pub(super) fn error() -> std::io::Error {
    crt_error(super::msvc::errno())
}

/// `getc`
pub(super) fn getc(u: Gc<Userdata>) -> Option<u8> {
    with(u, |f, os| f.getc(os))
}

/// `ungetc` of each byte; a refused one is lost, as in the C library.
pub(super) fn unget(u: Gc<Userdata>, bytes: &[u8]) {
    with(u, |f, _| {
        for &b in bytes {
            f.ungetc(b);
        }
    });
}

/// `fread`
pub(super) fn fread(u: Gc<Userdata>, n: usize) -> Vec<u8> {
    with(u, |f, os| f.fread(os, n))
}

/// `fwrite`: an error when it took fewer bytes.
pub(super) fn fwrite(u: Gc<Userdata>, bytes: &[u8]) -> std::io::Result<()> {
    if with(u, |f, os| f.fwrite(os, bytes)) == bytes.len() {
        Ok(())
    } else {
        Err(error())
    }
}

/// `fflush`
pub(super) fn fflush(u: Gc<Userdata>) -> std::io::Result<()> {
    if with(u, |f, os| f.fflush(os)) {
        Ok(())
    } else {
        Err(error())
    }
}

/// `fseek` then `ftell`, as `file:seek` does: the `ftell` result is given
/// even when it is -1.
pub(super) fn seek(u: Gc<Userdata>, op: usize, offset: i64) -> std::io::Result<i64> {
    with(u, |f, os| {
        if f.fseek(os, offset, op as u8) {
            Ok(f.ftell(os))
        } else {
            Err(error())
        }
    })
}

/// `clearerr`, and (5.4+) `errno = 0`, at the start of a read.
pub(super) fn begin_read(vm: &Vm, u: Gc<Userdata>) {
    with(u, |f, _| f.clearerr());
    if vm.version() >= LuaVersion::Lua54 {
        super::msvc::set_errno(0);
    }
}

/// `ferror` after the formats of a read.
pub(super) fn ferror(u: Gc<Userdata>) -> bool {
    with(u, |f, _| f.ferror())
}

/// `fclose`: whether flushing worked.
pub(super) fn fclose(u: Gc<Userdata>) -> std::io::Result<()> {
    if with(u, |f, os| f.fclose(os)) {
        Ok(())
    } else {
        Err(error())
    }
}

/// `setvbuf(f, NULL, mode, size)` (`mode`: 0 full, 1 line, 2 none).
pub(super) fn setvbuf(u: Gc<Userdata>, mode: u8, size: i64) -> bool {
    if mode != 2 && !(2..=i64::from(i32::MAX)).contains(&size) {
        invalid_parameter();
    }
    with(u, |f, os| f.setvbuf(os, mode, size as usize))
}

/// The C library's default handler for an invalid argument to one of its
/// functions ends the process at once (`__fastfail`), without flushing.
pub(crate) fn invalid_parameter() -> ! {
    std::process::exit(0xC000_0409_u32 as i32)
}
