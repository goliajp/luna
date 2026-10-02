//! Errors in the shape of `luaL_fileresult` / `luaL_execresult`, and the C-string and path conversions they share.

use super::*;

/// An errno luna reports itself. std reads a raw code as a Win32 error on
/// Windows, where 22 is "The device does not recognize the command."; the C
/// runtime's `strerror`, which PUC prints, says "Invalid argument".
pub(super) fn posix_error(code: i32) -> std::io::Error {
    #[cfg(windows)]
    {
        std::io::Error::other(PosixErrno(code))
    }
    #[cfg(not(windows))]
    {
        std::io::Error::from_raw_os_error(code)
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct PosixErrno(i32);

#[cfg(windows)]
impl std::fmt::Display for PosixErrno {
    // the MSVC C runtime's strerror texts for the codes luna raises
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            EBADF => "Bad file descriptor",
            ENOMEM => "Not enough space",
            EINVAL => "Invalid argument",
            ESPIPE => "Invalid seek",
            _ => "Unknown error",
        })
    }
}

#[cfg(windows)]
impl std::error::Error for PosixErrno {}

/// The errno of an error, as `luaL_fileresult` returns it.
fn errno(e: &std::io::Error) -> Option<i32> {
    #[cfg(windows)]
    if let Some(p) = e.get_ref().and_then(|r| r.downcast_ref::<PosixErrno>()) {
        return Some(p.0);
    }
    e.raw_os_error()
}

/// C `strerror` text of an OS error (std appends " (os error N)").
pub(crate) fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match e.raw_os_error() {
        Some(code) => s
            .strip_suffix(&format!(" (os error {code})"))
            .expect("std renders OS errors with an '(os error N)' suffix")
            .to_string(),
        None => s,
    }
}

/// `luaL_fileresult` for a failure: `(nil, "[fname: ]strerror", errno)`.
pub(crate) fn file_fail(vm: &mut Vm, fs: u32, fname: Option<&[u8]>, e: &std::io::Error) -> u32 {
    let vals = file_fail_values(vm, fname, e);
    vm.nat_return(fs, &vals)
}

pub(super) fn file_fail_values(
    vm: &mut Vm,
    fname: Option<&[u8]>,
    e: &std::io::Error,
) -> [Value; 3] {
    let mut msg = Vec::new();
    if let Some(n) = fname {
        msg.extend_from_slice(c_str(n));
        msg.extend_from_slice(b": ");
    }
    msg.extend_from_slice(strerror(e).as_bytes());
    let code = errno(e).map_or(0, i64::from);
    let m = Value::Str(vm.heap.intern(&msg));
    [Value::Nil, m, Value::Int(code)]
}

/// `luaL_fileresult` for success.
pub(super) fn file_ok(vm: &mut Vm, fs: u32) -> u32 {
    vm.nat_return(fs, &[Value::Bool(true)])
}

/// `luaL_execresult` on a waited-for child (5.2+): `(true|nil, "exit"|
/// "signal", code)`, or the file-result triple when waiting failed.
#[cfg(any(unix, windows))]
pub(crate) fn exec_result(
    vm: &mut Vm,
    fs: u32,
    status: std::io::Result<std::process::ExitStatus>,
) -> u32 {
    let status = match status {
        Ok(s) => s,
        Err(e) => return file_fail(vm, fs, None, &e),
    };
    let (what, code) = exit_status_breakdown(&status);
    let ok = if what == "exit" && code == 0 {
        Value::Bool(true)
    } else {
        Value::Nil
    };
    let w = Value::Str(vm.heap.intern(what.as_bytes()));
    vm.nat_return(fs, &[ok, w, Value::Int(code as i64)])
}

/// `l_inspectstat`: `WIFEXITED` → ("exit", status), `WIFSIGNALED` →
/// ("signal", signal). Windows has no signals.
#[cfg(any(unix, windows))]
pub(crate) fn exit_status_breakdown(status: &std::process::ExitStatus) -> (&'static str, i32) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return ("signal", sig);
        }
    }
    (
        "exit",
        status
            .code()
            .expect("a status that is not a signal carries a code"),
    )
}

// ---- paths ----

/// The part of a Lua string a C function sees as a file name: up to the
/// first NUL.
pub(crate) fn c_str(b: &[u8]) -> &[u8] {
    b.split(|&c| c == 0)
        .next()
        .expect("split yields a first piece")
}

/// A C file name (bytes up to the first NUL) as an OS path.
pub(crate) fn os_path(b: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(c_str(b)).into()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(c_str(b)).into_owned().into()
    }
}
