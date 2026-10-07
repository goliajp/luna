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

/// An errno spelled as the MSVC C runtime's `strerror` spells it.
#[derive(Debug)]
struct PosixErrno(i32);

impl std::fmt::Display for PosixErrno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const TEXT: [&str; 43] = [
            "No error",
            "Operation not permitted",
            "No such file or directory",
            "No such process",
            "Interrupted function call",
            "Input/output error",
            "No such device or address",
            "Arg list too long",
            "Exec format error",
            "Bad file descriptor",
            "No child processes",
            "Resource temporarily unavailable",
            "Not enough space",
            "Permission denied",
            "Bad address",
            "Unknown error",
            "Resource device",
            "File exists",
            "Improper link",
            "No such device",
            "Not a directory",
            "Is a directory",
            "Invalid argument",
            "Too many open files in system",
            "Too many open files",
            "Inappropriate I/O control operation",
            "Unknown error",
            "File too large",
            "No space left on device",
            "Invalid seek",
            "Read-only file system",
            "Too many links",
            "Broken pipe",
            "Domain error",
            "Result too large",
            "Unknown error",
            "Resource deadlock avoided",
            "Unknown error",
            "Filename too long",
            "No locks available",
            "Function not implemented",
            "Directory not empty",
            "Illegal byte sequence",
        ];
        f.write_str(
            usize::try_from(self.0)
                .ok()
                .and_then(|i| TEXT.get(i))
                .unwrap_or(&"Unknown error"),
        )
    }
}

impl std::error::Error for PosixErrno {}

/// The error a C call of the MSVC C library leaves in `errno`, with that
/// library's text, on every platform (for the streams of `Userdata::crt`).
pub(super) fn crt_error(code: i32) -> std::io::Error {
    std::io::Error::other(PosixErrno(code))
}

/// The errno of an error, as `luaL_fileresult` returns it. On Windows an
/// OS error is a Win32 code, which the C library maps to an errno.
fn errno(e: &std::io::Error) -> Option<i32> {
    if let Some(p) = e.get_ref().and_then(|r| r.downcast_ref::<PosixErrno>()) {
        return Some(p.0);
    }
    let code = e.raw_os_error();
    if cfg!(windows) {
        return code.map(|c| crate::cerrno::errno_of_win32(c as u32));
    }
    code
}

/// Set the process's `errno` to what the failed C call behind `e` left.
pub(crate) fn note_failure(e: &std::io::Error) {
    if let Some(c) = errno(e) {
        crate::cerrno::set(c);
    }
}

/// C `strerror` text of an OS error (std appends " (os error N)"); on
/// Windows the C library's text for the errno the error maps to.
pub(crate) fn strerror(e: &std::io::Error) -> String {
    if cfg!(windows)
        && let Some(c) = e.raw_os_error()
    {
        return PosixErrno(crate::cerrno::errno_of_win32(c as u32)).to_string();
    }
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
    note_failure(e);
    let mut msg = Vec::new();
    if let Some(n) = fname {
        msg.extend_from_slice(c_str(n));
        msg.extend_from_slice(b": ");
    }
    let code = errno(e).map_or(0, i64::from);
    if errno(e) == Some(0) && vm.version() >= LuaVersion::Lua54 {
        // 5.4's luaL_fileresult for an errno of 0
        msg.extend_from_slice(b"(no extra info)");
    } else {
        msg.extend_from_slice(strerror(e).as_bytes());
    }
    let m = Value::Str(vm.heap.intern(&msg));
    [Value::Nil, m, Value::Int(code)]
}

/// `errno = 0`, which 5.4 and later do before the C calls of most io and
/// os functions.
pub(crate) fn reset_errno(vm: &Vm) {
    if vm.version() >= LuaVersion::Lua54 {
        crate::cerrno::set(0);
    }
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

/// A C file name (bytes up to the first NUL) as an OS path: the bytes
/// themselves on Unix, through the ANSI code page on Windows.
pub(crate) fn os_path(b: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(c_str(b)).into()
    }
    #[cfg(windows)]
    {
        super::winfs::os_path(b)
    }
    #[cfg(not(any(unix, windows)))]
    {
        String::from_utf8_lossy(c_str(b)).into_owned().into()
    }
}

/// OS text (a path, an environment value) as the bytes a C program gets:
/// the bytes themselves on Unix, through the ANSI code page on Windows.
pub fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        super::winfs::narrow(s)
    }
    #[cfg(not(any(unix, windows)))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}
