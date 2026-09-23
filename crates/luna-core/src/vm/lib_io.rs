//! io library: `FILE*` handles over OS files, the default input/output
//! streams, and `io.popen`. Shaped per dialect after liolib.c 5.1–5.5 — the
//! file metatable's layout, what the operations return, how read formats
//! are spelled and how numbers are written all changed between versions.
//!
//! A handle's payload plays the part of C stdio's `FILE`: `write_buf` is its
//! output buffer, `read_buf[read_pos..]` its input buffer (which also holds
//! pushed-back bytes).

use std::io::{Read, Seek, SeekFrom, Write};

use crate::numeric::{self, FloatFmt, Num};
use crate::runtime::value::f2i_exact;
use crate::runtime::{FileHandle, Gc, Table, Userdata, UserdataPayload, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// `EINVAL` / `EBADF` / `ESPIPE`: the errno values stdio reports for the
/// failures luna detects itself rather than receiving from the OS.
const EINVAL: i32 = 22;
const EBADF: i32 = 9;
#[cfg(not(unix))]
const ESPIPE: i32 = 29;

/// `LUAL_BUFFERSIZE` for `setvbuf`'s default size; luna's buffers do not
/// take a size, but the argument is still checked.
const LUAL_BUFFERSIZE: i64 = 1024;

/// Bytes pulled from the OS per refill of a handle's input buffer.
const READ_CHUNK: usize = 4096;

pub(crate) fn open_io(vm: &mut Vm) {
    let v = vm.version();
    let io = vm.heap.new_table();
    for (name, f) in [
        ("close", io_close as crate::runtime::value::NativeFn),
        ("flush", io_flush),
        ("input", io_input),
        ("lines", io_lines),
        ("open", io_open),
        ("output", io_output),
        ("popen", io_popen),
        ("read", io_read),
        ("tmpfile", io_tmpfile),
        ("type", io_type),
        ("write", io_write),
    ] {
        put_native(vm, io, name, f);
    }
    // The method `close`: 5.1 and 5.2 register `io_close` itself (5.1 with an
    // environment that has no default output, so a missing argument checks
    // nil); 5.3 split it into `f_close`.
    let close: crate::runtime::value::NativeFn = match v {
        LuaVersion::Lua51 => f_close_51,
        LuaVersion::Lua52 => io_close,
        _ => f_close,
    };
    let methods: [(&str, crate::runtime::value::NativeFn); 7] = [
        ("close", close),
        ("flush", f_flush),
        ("lines", f_lines),
        ("read", f_read),
        ("seek", f_seek),
        ("setvbuf", f_setvbuf),
        ("write", f_write),
    ];
    // ≤5.3 keep the methods in the metatable itself (`__index = mt`); 5.4
    // moved them to a table of their own and added `__close`. `__name` comes
    // from `luaL_newmetatable`, which only sets it from 5.3 on.
    let mt = vm.heap.new_table();
    let index = if v >= LuaVersion::Lua54 {
        vm.heap.new_table()
    } else {
        mt
    };
    for (name, f) in methods {
        put_native(vm, index, name, f);
    }
    put(vm, mt, "__index", Value::Table(index));
    put_native(vm, mt, "__gc", f_gc);
    put_native(vm, mt, "__tostring", f_tostring);
    if v >= LuaVersion::Lua53 {
        let n = Value::Str(vm.heap.intern(b"FILE*"));
        put(vm, mt, "__name", n);
    }
    if v >= LuaVersion::Lua54 {
        // a to-be-closed file already shut by the user must not re-error,
        // hence the __gc body rather than f_close
        put_native(vm, mt, "__close", f_gc);
    }
    vm.barrier_back_table(index);
    vm.barrier_back_table(mt);
    vm.file_mt = Some(mt);

    for (name, fh) in [
        ("stdin", FileHandle::Stdin),
        ("stdout", FileHandle::Stdout),
        ("stderr", FileHandle::Stderr),
    ] {
        let writable = !matches!(fh, FileHandle::Stdin);
        let h = new_file(vm, fh, writable);
        put(vm, io, name, Value::Userdata(h));
        match name {
            "stdin" => vm.io_input = Some(h),
            "stdout" => vm.io_output = Some(h),
            _ => {}
        }
    }
    vm.set_global("io", Value::Table(io))
        .expect("stdlib registration");
    vm.barrier_back_table(io);
}

fn put(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let k = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, v)
        .expect("valid key");
}

fn put_native(vm: &mut Vm, t: Gc<Table>, k: &str, f: crate::runtime::value::NativeFn) {
    let fv = vm.native(f);
    put(vm, t, k, fv);
}

// ---- errors in the shape of luaL_fileresult / luaL_execresult ----

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

fn file_fail_values(vm: &mut Vm, fname: Option<&[u8]>, e: &std::io::Error) -> [Value; 3] {
    let mut msg = Vec::new();
    if let Some(n) = fname {
        msg.extend_from_slice(c_str(n));
        msg.extend_from_slice(b": ");
    }
    msg.extend_from_slice(strerror(e).as_bytes());
    let code = e.raw_os_error().map_or(0, i64::from);
    let m = Value::Str(vm.heap.intern(&msg));
    [Value::Nil, m, Value::Int(code)]
}

/// `luaL_fileresult` for success.
fn file_ok(vm: &mut Vm, fs: u32) -> u32 {
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

// ---- handles ----

/// A new handle carrying the `FILE*` metatable. Like `luaL_setmetatable` on
/// a metatable with `__gc`, this registers it for finalization, so a file
/// nobody closed is still flushed and closed when collected or at state
/// close. `writable` gives it a user-space output buffer.
fn new_file(vm: &mut Vm, fh: FileHandle, writable: bool) -> Gc<Userdata> {
    let u = vm.heap.new_userdata(UserdataPayload::File(fh), writable);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.set_metatable(vm.file_mt);
    vm.heap.register_finalizable_userdata(u);
    u
}

/// `luaL_testudata(L, i, LUA_FILEHANDLE)`: a userdata whose metatable is the
/// `FILE*` one (luna only gives that metatable to file payloads, but
/// `debug.setmetatable` can hand it to any userdata).
fn test_file(vm: &Vm, v: Value) -> Option<Gc<Userdata>> {
    match v {
        Value::Userdata(u)
            if matches!(u.payload, UserdataPayload::File(_))
                && u.metatable()
                    .zip(vm.file_mt)
                    .is_some_and(|(a, b)| a.ptr_eq(b)) =>
        {
            Some(u)
        }
        _ => None,
    }
}

/// `tolstream`: argument `i` must be a `FILE*`.
fn check_stream(vm: &mut Vm, a: Args, i: u32) -> Result<Gc<Userdata>, LuaError> {
    match test_file(vm, a.get(vm, i)) {
        Some(u) if !a.is_none(i) => Ok(u),
        _ => Err(argcheck::type_error(vm, a, i, "FILE*")),
    }
}

/// `tofile`: argument `i` must be an open `FILE*`.
fn check_open(vm: &mut Vm, a: Args, i: u32) -> Result<Gc<Userdata>, LuaError> {
    let u = check_stream(vm, a, i)?;
    if u.file().is_closed() {
        return Err(raise_str(vm, "attempt to use a closed file"));
    }
    Ok(u)
}

/// Drain every open handle's output buffer and stdout (C `fflush(NULL)`).
pub(crate) fn flush_all(vm: &mut Vm) {
    for u in vm.heap.finalizable_userdata() {
        if matches!(u.payload, UserdataPayload::File(ref fh) if !fh.is_closed()) {
            // like fflush(NULL), a stream that fails to flush does not stop
            // the others and reports nothing
            let _ = drain_write_buf(u);
        }
    }
    let _ = std::io::stdout().flush(); // same: fflush(NULL) reports nothing
}

// ---- closing ----

/// What closing a stream did (`aux_close` → `io_noclose` / `io_fclose` /
/// `io_pclose`).
enum Closed {
    /// a standard stream, left open
    Std,
    /// a regular file: the flush-on-close result
    File(std::io::Result<()>),
    /// a popen stream: the wait result
    #[cfg(any(unix, windows))]
    Pipe(std::io::Result<std::process::ExitStatus>),
}

fn close_stream(u: Gc<Userdata>) -> Closed {
    if u.file().is_std() {
        return Closed::Std;
    }
    let flushed = drain_write_buf(u);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    // dropping the handle closes the descriptor; for a pipe the child then
    // sees EOF before we wait for it
    *m.file_mut() = FileHandle::Closed;
    m.read_buf = Vec::new();
    m.read_pos = 0;
    #[cfg(any(unix, windows))]
    if let Some(mut child) = m.popen_child.take() {
        return Closed::Pipe(child.wait());
    }
    Closed::File(flushed)
}

fn push_closed(vm: &mut Vm, fs: u32, c: Closed) -> u32 {
    match c {
        Closed::Std => {
            let m = Value::Str(vm.heap.intern(b"cannot close standard file"));
            vm.nat_return(fs, &[Value::Nil, m])
        }
        Closed::File(Ok(())) => file_ok(vm, fs),
        Closed::File(Err(e)) => file_fail(vm, fs, None, &e),
        // 5.1's `lua_pclose` only tells whether pclose itself worked
        #[cfg(any(unix, windows))]
        Closed::Pipe(r) if vm.version() == LuaVersion::Lua51 => match r {
            Ok(_) => file_ok(vm, fs),
            Err(e) => file_fail(vm, fs, None, &e),
        },
        #[cfg(any(unix, windows))]
        Closed::Pipe(r) => exec_result(vm, fs, r),
    }
}

/// `io.close([file])`, also the 5.2 method: no argument means the default
/// output.
fn io_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = if nargs == 0 {
        let d = default_file(vm, Io::Output);
        if d.file().is_closed() {
            return Err(raise_str(vm, "attempt to use a closed file"));
        }
        d
    } else {
        check_open(vm, Args::new(fs, nargs), 0)?
    };
    let c = close_stream(u);
    Ok(push_closed(vm, fs, c))
}

/// 5.1's method `close` is `io_close` with an environment lacking the
/// default output, so without an argument it checks a nil.
fn f_close_51(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(vm, 1, "FILE* expected, got nil"));
    }
    f_close(vm, fs, nargs)
}

fn f_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    let c = close_stream(u);
    Ok(push_closed(vm, fs, c))
}

/// `__gc` (and 5.4's `__close`): close an open handle, ignoring the outcome.
fn f_gc(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_stream(vm, Args::new(fs, nargs), 0)?;
    if !u.file().is_closed() {
        let _ = close_stream(u); // PUC's f_gc drops aux_close's results
    }
    Ok(vm.nat_return(fs, &[]))
}

fn f_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_stream(vm, Args::new(fs, nargs), 0)?;
    let s = if u.file().is_closed() {
        "file (closed)".to_string()
    } else {
        format!("file ({:p})", u.as_ptr())
    };
    let v = Value::Str(vm.heap.intern(s.as_bytes()));
    Ok(vm.nat_return(fs, &[v]))
}

fn io_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    let r = match test_file(vm, v) {
        None => Value::Nil,
        Some(u) if u.file().is_closed() => Value::Str(vm.heap.intern(b"closed file")),
        Some(_) => Value::Str(vm.heap.intern(b"file")),
    };
    Ok(vm.nat_return(fs, &[r]))
}

// ---- opening ----

/// How `fopen` opens a mode; `None` when the mode is not one `fopen`
/// accepts (EINVAL).
fn fopen_options(mode: &[u8]) -> Option<(std::fs::OpenOptions, bool)> {
    let mut o = std::fs::OpenOptions::new();
    let first = *mode.first()?;
    let rest = &mode[1..];
    let plus = rest.contains(&b'+');
    // macOS fopen honours 'x' (O_EXCL) and ignores the other extra letters
    let excl = rest.contains(&b'x');
    let writable = match first {
        b'r' => {
            o.read(true).write(plus);
            plus
        }
        b'w' => {
            o.write(true).read(plus).truncate(true);
            if excl {
                o.create_new(true);
            } else {
                o.create(true);
            }
            true
        }
        b'a' => {
            o.append(true).read(plus);
            if excl {
                o.create_new(true);
            } else {
                o.create(true);
            }
            true
        }
        _ => return None,
    };
    Some((o, writable))
}

/// `l_checkmode`: 5.2 accepts `[rwa]%+?b?`, 5.3+ `[rwa]%+?b*`; 5.1 checks
/// nothing and lets `fopen` decide.
fn mode_ok(v: LuaVersion, mode: &[u8]) -> bool {
    let Some((&first, rest)) = mode.split_first() else {
        return false;
    };
    if !b"rwa".contains(&first) {
        return false;
    }
    let rest = rest.strip_prefix(b"+").unwrap_or(rest);
    match v {
        LuaVersion::Lua52 => rest.is_empty() || rest == b"b",
        _ => rest.iter().all(|&c| c == b'b'),
    }
}

fn open_file(name: &[u8], mode: &[u8]) -> std::io::Result<(std::fs::File, bool)> {
    let (o, writable) =
        fopen_options(c_str(mode)).ok_or_else(|| std::io::Error::from_raw_os_error(EINVAL))?;
    Ok((o.open(os_path(name))?, writable))
}

fn io_open(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let name = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let mode = match argcheck::opt_string(vm, a, 1)? {
        Some(m) => m.as_bytes().to_vec(),
        None => b"r".to_vec(),
    };
    if vm.version() >= LuaVersion::Lua52 && !mode_ok(vm.version(), &mode) {
        return Err(arg_error(vm, 2, "invalid mode"));
    }
    match open_file(&name, &mode) {
        Ok((f, writable)) => {
            let u = new_file(vm, FileHandle::File(f), writable);
            Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
        }
        Err(e) => Ok(file_fail(vm, fs, Some(&name), &e)),
    }
}

fn io_tmpfile(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("lua_tmp_{}_{n}", std::process::id()));
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => return Ok(file_fail(vm, fs, None, &e)),
    };
    // tmpfile(3) leaves no name behind; the open handle keeps the file.
    if let Err(e) = std::fs::remove_file(&path) {
        return Ok(file_fail(vm, fs, None, &e));
    }
    let u = new_file(vm, FileHandle::File(file), true);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// `io.popen(prog [, mode])`: a `/bin/sh -c prog` child with its stdout
/// (`"r"`) or stdin (`"w"`) as the stream.
#[cfg(any(unix, windows))]
fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let prog = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let mode = match argcheck::opt_string(vm, a, 1)? {
        Some(m) => m.as_bytes().to_vec(),
        None => b"r".to_vec(),
    };
    // 5.3+ check the mode (`l_checkmodep`); before that popen(3) did, and
    // refuses anything but r/w with EINVAL. ("r+"/"w+", a two-way stream on
    // BSD popen, is not provided.)
    let read = match c_str(&mode) {
        b"r" => true,
        b"w" => false,
        _ if vm.version() >= LuaVersion::Lua53 => return Err(arg_error(vm, 2, "invalid mode")),
        _ => {
            let e = std::io::Error::from_raw_os_error(EINVAL);
            return Ok(file_fail(vm, fs, Some(&prog), &e));
        }
    };
    // `l_popen` flushes every output stream first (`fflush(NULL)`), so the
    // child sees what the parent wrote before it.
    flush_all(vm);
    let mut cmd = shell_command(&prog);
    if read {
        cmd.stdout(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ok(file_fail(vm, fs, Some(&prog), &e)),
    };
    let file = if read {
        pipe_file(child.stdout.take().expect("stdout was piped"))
    } else {
        pipe_file(child.stdin.take().expect("stdin was piped"))
    };
    let u = new_file(vm, FileHandle::File(file), !read);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.popen_child = Some(child);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// A child's pipe end as a plain file, so reads and writes share one path.
#[cfg(unix)]
fn pipe_file(p: impl Into<std::os::fd::OwnedFd>) -> std::fs::File {
    std::fs::File::from(p.into())
}

#[cfg(windows)]
fn pipe_file(p: impl Into<std::os::windows::io::OwnedHandle>) -> std::fs::File {
    std::fs::File::from(p.into())
}

/// Targets without processes (`wasm32-wasip1`): the ISO C `l_popen`, which
/// raises "'popen' not supported" after the argument checks.
#[cfg(not(any(unix, windows)))]
fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    argcheck::check_string(vm, a, 0)?;
    argcheck::opt_string(vm, a, 1)?;
    Err(raise_str(vm, "'popen' not supported"))
}

/// The shell `system(3)` and `popen(3)` run a command through.
#[cfg(any(unix, windows))]
pub(crate) fn shell_command(cmd: &[u8]) -> std::process::Command {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut c = std::process::Command::new("/bin/sh");
        c.arg("-c").arg(std::ffi::OsStr::from_bytes(c_str(cmd)));
        c
    }
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C")
            .arg(String::from_utf8_lossy(c_str(cmd)).into_owned());
        c
    }
}

// ---- default streams ----

#[derive(Clone, Copy)]
enum Io {
    Input,
    Output,
}

fn default_file(vm: &Vm, which: Io) -> Gc<Userdata> {
    match which {
        Io::Input => vm.io_input,
        Io::Output => vm.io_output,
    }
    .expect("default streams are set when io opens")
}

/// `getiofile`: the default stream, which must still be open.
fn get_io_file(vm: &mut Vm, which: Io) -> Result<Gc<Userdata>, LuaError> {
    let u = default_file(vm, which);
    if u.file().is_closed() {
        let what = match which {
            Io::Input => "input",
            Io::Output => "output",
        };
        let adj = if vm.version() >= LuaVersion::Lua54 {
            "default"
        } else {
            "standard"
        };
        return Err(raise_str(vm, &format!("{adj} {what} file is closed")));
    }
    Ok(u)
}

/// `g_iofile`: set the default stream from a file name or a handle, then
/// return it.
fn g_iofile(vm: &mut Vm, fs: u32, nargs: u32, which: Io) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if !a.is_none_or_nil(vm, 0) {
        let v = a.get(vm, 0);
        let u = match argcheck::to_str_bytes(vm, v) {
            Some(name) => {
                let mode: &[u8] = match which {
                    Io::Input => b"r",
                    Io::Output => b"w",
                };
                open_checked(vm, &name, mode)?
            }
            None => check_open(vm, a, 0)?,
        };
        match which {
            Io::Input => vm.io_input = Some(u),
            Io::Output => vm.io_output = Some(u),
        }
    }
    let cur = default_file(vm, which);
    Ok(vm.nat_return(fs, &[Value::Userdata(cur)]))
}

/// `opencheck` (5.2+) / 5.1's `fileerror`: open or raise.
fn open_checked(vm: &mut Vm, name: &[u8], mode: &[u8]) -> Result<Gc<Userdata>, LuaError> {
    match open_file(name, mode) {
        Ok((f, writable)) => Ok(new_file(vm, FileHandle::File(f), writable)),
        Err(e) => {
            let n = String::from_utf8_lossy(c_str(name)).into_owned();
            let err = strerror(&e);
            Err(if vm.version() == LuaVersion::Lua51 {
                arg_error(vm, 1, &format!("{n}: {err}"))
            } else {
                raise_str(vm, &format!("cannot open file '{n}' ({err})"))
            })
        }
    }
}

fn io_input(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    g_iofile(vm, fs, nargs, Io::Input)
}

fn io_output(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    g_iofile(vm, fs, nargs, Io::Output)
}

// ---- the stream: buffered bytes over the OS handle ----

/// Refill the input buffer; `false` at end of file.
fn fill(u: Gc<Userdata>) -> std::io::Result<bool> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    let mut chunk = vec![0u8; READ_CHUNK];
    let n = match m.file_mut() {
        FileHandle::File(f) => f.read(&mut chunk)?,
        FileHandle::Stdin => std::io::stdin().read(&mut chunk)?,
        // stdout/stderr are write-only streams
        FileHandle::Stdout | FileHandle::Stderr => {
            return Err(std::io::Error::from_raw_os_error(EBADF));
        }
        FileHandle::Closed => unreachable!("reads check the stream is open"),
    };
    chunk.truncate(n);
    m.read_buf = chunk;
    m.read_pos = 0;
    Ok(n > 0)
}

/// `getc`.
fn getc(u: Gc<Userdata>) -> std::io::Result<Option<u8>> {
    if u.read_pos >= u.read_buf.len() && !fill(u)? {
        return Ok(None);
    }
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    let b = m.read_buf[m.read_pos];
    m.read_pos += 1;
    Ok(Some(b))
}

/// `ungetc` of any number of bytes: the next reads return `bytes` first.
fn unget(u: Gc<Userdata>, bytes: &[u8]) {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    if m.read_pos >= bytes.len() && m.read_buf[m.read_pos - bytes.len()..m.read_pos] == *bytes {
        m.read_pos -= bytes.len();
        return;
    }
    let mut buf = bytes.to_vec();
    buf.extend_from_slice(&m.read_buf[m.read_pos..]);
    m.read_buf = buf;
    m.read_pos = 0;
}

/// Bytes buffered ahead of the logical position.
fn read_ahead(u: Gc<Userdata>) -> i64 {
    (u.read_buf.len() - u.read_pos) as i64
}

/// Give back read-ahead before the position is used for something else
/// (a write, a seek): the OS position is that far past the logical one.
fn unread_ahead(u: Gc<Userdata>) -> std::io::Result<()> {
    let ahead = read_ahead(u);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    if ahead > 0
        && let FileHandle::File(f) = m.file_mut()
    {
        f.seek(SeekFrom::Current(-ahead))?;
    }
    m.read_buf = Vec::new();
    m.read_pos = 0;
    Ok(())
}

/// Write `bytes` straight to the OS handle.
fn write_to(u: Gc<Userdata>, bytes: &[u8]) -> std::io::Result<()> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.write_all(bytes),
        FileHandle::Stdout => std::io::stdout().write_all(bytes),
        FileHandle::Stderr => std::io::stderr().write_all(bytes),
        FileHandle::Stdin => Err(std::io::Error::from_raw_os_error(EBADF)),
        FileHandle::Closed => unreachable!("writes check the stream is open"),
    }
}

/// Drain the output buffer to the OS. The buffer is emptied either way: C
/// stdio drops what it failed to write and reports the error.
fn drain_write_buf(u: Gc<Userdata>) -> std::io::Result<()> {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let buf = std::mem::take(&mut unsafe { u.as_mut() }.write_buf);
    if buf.is_empty() {
        return Ok(());
    }
    write_to(u, &buf)
}

/// Put `bytes` on the stream through its buffering mode.
fn put_bytes(u: Gc<Userdata>, bytes: &[u8]) -> std::io::Result<()> {
    if !matches!(u.file(), FileHandle::File(_)) || !u.writable {
        // standard streams are buffered by std; a read-only file fails here
        return write_to(u, bytes);
    }
    unread_ahead(u)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    match m.buf_mode {
        BUF_NO => write_to(u, bytes),
        BUF_LINE => {
            m.write_buf.extend_from_slice(bytes);
            match m.write_buf.iter().rposition(|&b| b == b'\n') {
                Some(nl) => {
                    let out: Vec<u8> = m.write_buf.drain(..=nl).collect();
                    write_to(u, &out)
                }
                None => Ok(()),
            }
        }
        _ => {
            m.write_buf.extend_from_slice(bytes);
            Ok(())
        }
    }
}

/// `setvbuf` modes as kept in `Userdata::buf_mode`.
const BUF_FULL: u8 = 0;
const BUF_LINE: u8 = 1;
const BUF_NO: u8 = 2;

fn flush_stream(u: Gc<Userdata>) -> std::io::Result<()> {
    drain_write_buf(u)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { u.as_mut() }.file_mut() {
        FileHandle::File(f) => f.flush(),
        FileHandle::Stdout => std::io::stdout().flush(),
        FileHandle::Stderr => std::io::stderr().flush(),
        FileHandle::Stdin | FileHandle::Closed => Ok(()),
    }
}

// ---- writing ----

/// How the dialect writes a number: ≤5.2 `%.14g`; 5.3/5.4 `%lld` for an
/// integer and `%.14g` for a float (so no ".0"); 5.5 converts as tostring.
fn number_text(vm: &Vm, n: Num) -> Vec<u8> {
    let fmt = match vm.version() {
        LuaVersion::Lua55 => vm.float_fmt(),
        _ => FloatFmt::Legacy14,
    };
    numeric::num_to_string_for(n, fmt).into_bytes()
}

/// `g_write`: write `vals` in order and give the dialect's result.
fn g_write(vm: &mut Vm, fs: u32, u: Gc<Userdata>, args: Args, first: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let mut total: i64 = 0;
    let mut failure: Option<std::io::Error> = None;
    for i in first..args.n {
        let bytes = match args.get(vm, i) {
            Value::Int(x) => number_text(vm, Num::Int(x)),
            Value::Float(f) => number_text(vm, Num::Float(f)),
            _ => argcheck::check_string(vm, args, i)?.as_bytes().to_vec(),
        };
        // ≤5.4 stop writing after a failure but still check the remaining
        // arguments; 5.5 returns at the first failure
        if failure.is_some() {
            continue;
        }
        match put_bytes(u, &bytes) {
            Ok(()) => total += bytes.len() as i64,
            Err(e) if v >= LuaVersion::Lua55 => {
                let mut vals = file_fail_values(vm, None, &e).to_vec();
                vals.push(Value::Int(total));
                return Ok(vm.nat_return(fs, &vals));
            }
            Err(e) => failure = Some(e),
        }
    }
    Ok(match failure {
        Some(e) => file_fail(vm, fs, None, &e),
        // 5.1 reports success as true; 5.2+ return the file
        None if v == LuaVersion::Lua51 => file_ok(vm, fs),
        None => vm.nat_return(fs, &[Value::Userdata(u)]),
    })
}

fn io_write(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Output)?;
    g_write(vm, fs, u, Args::new(fs, nargs), 0)
}

fn f_write(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let u = check_open(vm, a, 0)?;
    g_write(vm, fs, u, a, 1)
}

fn io_flush(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Output)?;
    Ok(match flush_stream(u) {
        Ok(()) => file_ok(vm, fs),
        Err(e) => file_fail(vm, fs, None, &e),
    })
}

fn f_flush(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    Ok(match flush_stream(u) {
        Ok(()) => file_ok(vm, fs),
        Err(e) => file_fail(vm, fs, None, &e),
    })
}

// ---- seek / setvbuf ----

fn f_seek(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let u = check_open(vm, a, 0)?;
    let op = argcheck::check_option(vm, a, 1, Some("cur"), &["set", "cur", "end"])?;
    let offset = if vm.version() == LuaVersion::Lua52 {
        // 5.2 reads a float and requires it to survive the cast to off_t
        let p3 = argcheck::opt_number(vm, a, 2, 0.0)?;
        let off = p3 as i64;
        if off as f64 != p3 {
            return Err(arg_error(vm, 3, "not an integer in proper range"));
        }
        off
    } else {
        argcheck::opt_integer(vm, a, 2, 0)?
    };
    match seek_stream(u, op, offset) {
        Ok(pos) if vm.version() == LuaVersion::Lua52 => {
            Ok(vm.nat_return(fs, &[Value::Float(pos as f64)]))
        }
        Ok(pos) => Ok(vm.nat_return(fs, &[Value::Int(pos as i64)])),
        Err(e) => Ok(file_fail(vm, fs, None, &e)),
    }
}

/// `fseek` + `ftell`: flush pending output, give back read-ahead, move.
fn seek_stream(u: Gc<Userdata>, op: usize, offset: i64) -> std::io::Result<u64> {
    drain_write_buf(u)?;
    let ahead = read_ahead(u);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let m = unsafe { u.as_mut() };
    let from = match op {
        0 if offset < 0 => return Err(std::io::Error::from_raw_os_error(EINVAL)),
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset - ahead),
        _ => SeekFrom::End(offset),
    };
    let pos = match m.file_mut() {
        FileHandle::File(f) => f.seek(from)?,
        std_stream => seek_std(std_stream, from)?,
    };
    m.read_buf = Vec::new();
    m.read_pos = 0;
    Ok(pos)
}

/// Seek a standard stream through a duplicate of its descriptor, which
/// shares the offset (and fails with ESPIPE on a terminal or pipe).
#[cfg(unix)]
fn seek_std(fh: &FileHandle, from: SeekFrom) -> std::io::Result<u64> {
    use std::os::fd::AsFd;
    let fd = match fh {
        FileHandle::Stdin => std::io::stdin().as_fd().try_clone_to_owned()?,
        FileHandle::Stdout => std::io::stdout().as_fd().try_clone_to_owned()?,
        FileHandle::Stderr => std::io::stderr().as_fd().try_clone_to_owned()?,
        FileHandle::File(_) | FileHandle::Closed => unreachable!("only standard streams"),
    };
    std::fs::File::from(fd).seek(from)
}

#[cfg(not(unix))]
fn seek_std(_fh: &FileHandle, _from: SeekFrom) -> std::io::Result<u64> {
    Err(std::io::Error::from_raw_os_error(ESPIPE))
}

fn f_setvbuf(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let u = check_open(vm, a, 0)?;
    let op = argcheck::check_option(vm, a, 1, None, &["no", "full", "line"])?;
    argcheck::opt_integer(vm, a, 2, LUAL_BUFFERSIZE)?;
    let mode = [BUF_NO, BUF_FULL, BUF_LINE][op];
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.buf_mode = mode;
    if mode == BUF_NO
        && let Err(e) = drain_write_buf(u)
    {
        return Ok(file_fail(vm, fs, None, &e));
    }
    Ok(file_ok(vm, fs))
}

// ---- reading ----

/// Outcome of `g_read`: the values (the last one nil when a format failed),
/// or the I/O error that ends a read (`ferror`).
enum ReadOut {
    Values(Vec<Value>),
    Error(std::io::Error),
}

/// One read format, parsed.
enum Fmt {
    Count(i64),
    Number,
    Line { keep_nl: bool },
    All,
}

/// Parse read format `fmt`, the argument numbered `argno` in errors.
fn parse_format(vm: &mut Vm, fmt: Value, argno: u32) -> Result<Fmt, LuaError> {
    let v = vm.version();
    match fmt {
        Value::Int(n) => return Ok(Fmt::Count(n)),
        Value::Float(f) if v <= LuaVersion::Lua52 => return Ok(Fmt::Count(f as i64)),
        Value::Float(f) => {
            return f2i_exact(f)
                .map(Fmt::Count)
                .ok_or_else(|| arg_error(vm, argno, "number has no integer representation"));
        }
        _ => {}
    }
    let spec = match fmt {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ if v <= LuaVersion::Lua52 => return Err(arg_error(vm, argno, "invalid option")),
        _ => {
            let tn = argcheck::typename_of(vm, fmt);
            return Err(arg_error(vm, argno, &format!("string expected, got {tn}")));
        }
    };
    // ≤5.2 require the '*'; 5.3 made it optional
    let body = match spec.strip_prefix(b"*") {
        Some(b) => b,
        None if v <= LuaVersion::Lua52 => return Err(arg_error(vm, argno, "invalid option")),
        None => &spec,
    };
    Ok(match body.first() {
        Some(b'n') => Fmt::Number,
        Some(b'l') => Fmt::Line { keep_nl: false },
        Some(b'L') if v >= LuaVersion::Lua52 => Fmt::Line { keep_nl: true },
        Some(b'a') => Fmt::All,
        _ => return Err(arg_error(vm, argno, "invalid format")),
    })
}

/// `g_read`: apply `fmts` in order until one fails. With no formats, read a
/// line. `argno0` numbers the first format in argument errors.
fn g_read(vm: &mut Vm, u: Gc<Userdata>, fmts: &[Value], argno0: u32) -> Result<ReadOut, LuaError> {
    // stdio needs a flush between writing and reading the same stream
    if let Err(e) = drain_write_buf(u) {
        return Ok(ReadOut::Error(e));
    }
    if fmts.is_empty() {
        return Ok(match read_line(vm, u, false) {
            Ok(v) => ReadOut::Values(vec![v]),
            Err(e) => ReadOut::Error(e),
        });
    }
    let mut out = Vec::with_capacity(fmts.len());
    for (i, &f) in fmts.iter().enumerate() {
        let fmt = parse_format(vm, f, argno0 + i as u32)?;
        let r = match fmt {
            Fmt::Count(n) => read_count(vm, u, n)?,
            Fmt::Number => read_number(vm, u),
            Fmt::Line { keep_nl } => read_line(vm, u, keep_nl),
            Fmt::All => read_all(vm, u),
        };
        match r {
            Ok(v) => {
                let stop = v.is_nil();
                out.push(v);
                if stop {
                    break;
                }
            }
            Err(e) => return Ok(ReadOut::Error(e)),
        }
    }
    Ok(ReadOut::Values(out))
}

fn push_read(vm: &mut Vm, fs: u32, r: ReadOut) -> u32 {
    match r {
        ReadOut::Values(vals) => vm.nat_return(fs, &vals),
        ReadOut::Error(e) => file_fail(vm, fs, None, &e),
    }
}

fn io_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Input)?;
    let fmts: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let r = g_read(vm, u, &fmts, 1)?;
    Ok(push_read(vm, fs, r))
}

fn f_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let r = g_read(vm, u, &fmts, 2)?;
    Ok(push_read(vm, fs, r))
}

/// `BUFSIZ`, the size of the chunks ≤5.2 read a line in (`LUAL_BUFFERSIZE`).
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
const BUFSIZ: usize = 1024;
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
)))]
const BUFSIZ: usize = 8192;

/// `read_line`: up to and excluding (`keep_nl`: including) the newline; nil
/// when nothing at all was read.
fn read_line(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> std::io::Result<Value> {
    if vm.version() <= LuaVersion::Lua52 {
        return read_line_fgets(vm, u, keep_nl);
    }
    let mut buf = Vec::new();
    let mut got_nl = false;
    while let Some(c) = getc(u)? {
        if c == b'\n' {
            got_nl = true;
            if keep_nl {
                buf.push(c);
            }
            break;
        }
        buf.push(c);
    }
    Ok(if got_nl || !buf.is_empty() {
        Value::Str(vm.heap.intern(&buf))
    } else {
        Value::Nil
    })
}

/// ≤5.2's `read_line` reads with `fgets` and measures each chunk with
/// `strlen`: a NUL cuts the chunk short there, and a newline after it is
/// lost, so the line runs on into the next.
fn read_line_fgets(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> std::io::Result<Value> {
    let mut out = Vec::new();
    loop {
        let mut chunk = Vec::new();
        while chunk.len() < BUFSIZ - 1 {
            match getc(u)? {
                Some(c) => {
                    chunk.push(c);
                    if c == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        if chunk.is_empty() {
            return Ok(if out.is_empty() {
                Value::Nil
            } else {
                Value::Str(vm.heap.intern(&out))
            });
        }
        // strlen: up to the first NUL, the whole chunk when there is none
        let len = chunk.iter().position(|&b| b == 0).unwrap_or(chunk.len());
        if len == 0 || chunk[len - 1] != b'\n' {
            out.extend_from_slice(&chunk[..len]);
        } else {
            let end = if keep_nl { len } else { len - 1 };
            out.extend_from_slice(&chunk[..end]);
            return Ok(Value::Str(vm.heap.intern(&out)));
        }
    }
}

/// `read_all`: never fails (an empty string at end of file).
fn read_all(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    let mut buf = Vec::new();
    loop {
        buf.extend_from_slice(&u.read_buf[u.read_pos..]);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { u.as_mut() }.read_pos = u.read_buf.len();
        if !fill(u)? {
            break;
        }
    }
    Ok(Value::Str(vm.heap.intern(&buf)))
}

/// Sizes no allocator grants; PUC's buffer for them fails before reading.
const UNALLOCATABLE: u64 = 1 << 47;

/// A byte-count format: `0` tests for end of file; `n` reads up to `n`
/// bytes (nil when none were left). A negative count is a huge `size_t`.
fn read_count(vm: &mut Vm, u: Gc<Userdata>, n: i64) -> Result<std::io::Result<Value>, LuaError> {
    let size = n as u64;
    if size == 0 {
        return Ok(test_eof(vm, u));
    }
    // 5.1 reads in chunks, so any size works; 5.2+ size one buffer for the
    // whole request, which the allocator refuses for absurd sizes
    if size >= UNALLOCATABLE && vm.version() >= LuaVersion::Lua52 {
        return Err(match vm.version() {
            LuaVersion::Lua52 if size > u64::MAX - 64 => {
                vm.plain_err("memory allocation error: block too big")
            }
            LuaVersion::Lua53 => raise_str(vm, "not enough memory for buffer allocation"),
            LuaVersion::Lua55 if size >= i64::MAX as u64 => {
                raise_str(vm, "resulting string too large")
            }
            _ => vm.plain_err("not enough memory"),
        });
    }
    let mut buf = Vec::new();
    while (buf.len() as u64) < size {
        let want = (size - buf.len() as u64) as usize;
        if u.read_pos >= u.read_buf.len() {
            match fill(u) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => return Ok(Err(e)),
            }
        }
        let take = want.min(u.read_buf.len() - u.read_pos);
        buf.extend_from_slice(&u.read_buf[u.read_pos..u.read_pos + take]);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { u.as_mut() }.read_pos += take;
    }
    Ok(Ok(if buf.is_empty() {
        Value::Nil
    } else {
        Value::Str(vm.heap.intern(&buf))
    }))
}

/// `test_eof`: "" if a byte is left, nil at end of file.
fn test_eof(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    Ok(match getc(u)? {
        Some(c) => {
            unget(u, &[c]);
            Value::Str(vm.heap.intern(b""))
        }
        None => Value::Nil,
    })
}

fn read_number(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    if vm.version() <= LuaVersion::Lua52 {
        return scan_double(u);
    }
    let buf = read_numeral(u)?;
    Ok(match numeric::str2num(&buf, true, true) {
        Some(Num::Int(i)) => Value::Int(i),
        Some(Num::Float(f)) => Value::Float(f),
        None => Value::Nil,
    })
}

/// ≤5.2 read numbers with `fscanf("%lf")`. The BSD scanner converts the
/// longest prefix that is a valid floating-point numeral and pushes back
/// every byte after it; with no valid prefix, it pushes everything back.
fn scan_double(u: Gc<Userdata>) -> std::io::Result<Value> {
    let mut c = getc(u)?;
    while matches!(c, Some(b) if is_c_space(b)) {
        c = getc(u)?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut commit = 0; // length of the longest complete numeral in buf
    let mut state = Scan::Start;
    while let Some(b) = c {
        let next = scan_step(state, b, &buf);
        let Some((st, complete)) = next else { break };
        buf.push(b);
        if complete {
            commit = buf.len();
        }
        state = st;
        c = getc(u)?;
    }
    if let Some(b) = c {
        buf.push(b);
    }
    unget(u, &buf[commit..]);
    if commit == 0 {
        return Ok(Value::Nil);
    }
    Ok(Value::Float(parse_c_double(&buf[..commit])))
}

fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

#[derive(Clone, Copy, PartialEq)]
enum Scan {
    Start,
    Sign,
    Zero,
    Int,
    Dot,
    Frac,
    ExpMark,
    ExpSign,
    ExpDigits,
    HexX,
    HexInt,
    HexDot,
    HexFrac,
    Word,
    NanOpen,
    Done,
}

/// One step of the `%lf` scanner: the state after `b`, and whether the
/// bytes so far form a complete numeral. `None` when `b` cannot continue.
fn scan_step(s: Scan, b: u8, buf: &[u8]) -> Option<(Scan, bool)> {
    let digits = |st| Some((st, true));
    match s {
        Scan::Start | Scan::Sign => match b {
            b'+' | b'-' if s == Scan::Start => Some((Scan::Sign, false)),
            b'0' => digits(Scan::Zero),
            b'1'..=b'9' => digits(Scan::Int),
            b'.' => Some((Scan::Dot, false)),
            b'i' | b'I' | b'n' | b'N' => Some((Scan::Word, false)),
            _ => None,
        },
        Scan::Zero if matches!(b, b'x' | b'X') => Some((Scan::HexX, false)),
        Scan::Zero | Scan::Int => match b {
            b'0'..=b'9' => digits(Scan::Int),
            b'.' => digits(Scan::Frac),
            b'e' | b'E' => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::Dot => match b {
            b'0'..=b'9' => digits(Scan::Frac),
            _ => None,
        },
        Scan::Frac => match b {
            b'0'..=b'9' => digits(Scan::Frac),
            b'e' | b'E' => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::ExpMark => match b {
            b'+' | b'-' => Some((Scan::ExpSign, false)),
            b'0'..=b'9' => digits(Scan::ExpDigits),
            _ => None,
        },
        Scan::ExpSign | Scan::ExpDigits => match b {
            b'0'..=b'9' => digits(Scan::ExpDigits),
            _ => None,
        },
        Scan::HexX => match b {
            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => digits(Scan::HexInt),
            b'.' => Some((Scan::HexDot, false)),
            _ => None,
        },
        Scan::HexInt => match b {
            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => digits(Scan::HexInt),
            b'.' => digits(Scan::HexFrac),
            b'p' | b'P' => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::HexDot | Scan::HexFrac => match b {
            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => digits(Scan::HexFrac),
            b'p' | b'P' if s == Scan::HexFrac => Some((Scan::ExpMark, false)),
            _ => None,
        },
        Scan::Word => {
            // "inf", "infinity", "nan", compared case-insensitively
            let word: Vec<u8> = buf
                .iter()
                .skip_while(|&&c| c == b'+' || c == b'-')
                .map(u8::to_ascii_lowercase)
                .chain(std::iter::once(b.to_ascii_lowercase()))
                .collect();
            if b"infinity".starts_with(&word) {
                Some((Scan::Word, word == b"inf" || word == b"infinity"))
            } else if b"nan".starts_with(&word) {
                Some((Scan::Word, word == b"nan"))
            } else if word == b"nan(" {
                Some((Scan::NanOpen, false))
            } else {
                None
            }
        }
        Scan::NanOpen => match b {
            b')' => Some((Scan::Done, true)),
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' => Some((Scan::NanOpen, false)),
            _ => None,
        },
        Scan::Done => None,
    }
}

/// `strtod` on a complete numeral from `scan_double`.
fn parse_c_double(s: &[u8]) -> f64 {
    let (neg, body) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    let mag = if lower.starts_with(b"inf") {
        f64::INFINITY
    } else if lower.starts_with(b"nan") {
        f64::NAN
    } else if lower.starts_with(b"0x") {
        match numeric::str2num(body, false, true) {
            Some(n) => n.as_f64(),
            None => unreachable!("the scanner only commits valid hex numerals"),
        }
    } else {
        std::str::from_utf8(body)
            .expect("decimal numerals are ASCII")
            .parse::<f64>()
            .expect("the scanner only commits valid decimal numerals")
    };
    if neg { -mag } else { mag }
}

/// Cap on a numeral's length for 5.3+'s reader (`L_MAXLENNUM`).
const L_MAXLENNUM: usize = 200;

/// 5.3+ `read_number`'s state: the look-ahead byte and the saved prefix.
struct Rn {
    buf: Vec<u8>,
    c: Option<u8>,
}

/// `nextc`: keep the look-ahead byte and read the next. Past
/// `L_MAXLENNUM` the numeral is invalidated and reading stops.
fn rn_next(rn: &mut Rn, u: Gc<Userdata>) -> std::io::Result<bool> {
    if rn.buf.len() >= L_MAXLENNUM {
        rn.buf.clear();
        return Ok(false);
    }
    if let Some(b) = rn.c {
        rn.buf.push(b);
    }
    rn.c = getc(u)?;
    Ok(true)
}

/// `test2`: take the look-ahead byte if it is one of `set`.
fn rn_test(rn: &mut Rn, u: Gc<Userdata>, set: &[u8]) -> std::io::Result<bool> {
    if matches!(rn.c, Some(c) if set.contains(&c)) {
        return rn_next(rn, u);
    }
    Ok(false)
}

/// `readdigits`.
fn rn_digits(rn: &mut Rn, u: Gc<Userdata>, hex: bool) -> std::io::Result<u32> {
    let mut count = 0;
    while matches!(rn.c, Some(c) if if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() })
        && rn_next(rn, u)?
    {
        count += 1;
    }
    Ok(count)
}

/// 5.3+ `read_number`'s scan: the longest prefix following a fixed numeral
/// grammar, with the first byte that does not fit pushed back.
fn read_numeral(u: Gc<Userdata>) -> std::io::Result<Vec<u8>> {
    let mut c = getc(u)?;
    while matches!(c, Some(b) if is_c_space(b)) {
        c = getc(u)?;
    }
    let mut rn = Rn { buf: Vec::new(), c };
    let mut count = 0;
    let mut hex = false;
    rn_test(&mut rn, u, b"-+")?;
    if rn_test(&mut rn, u, b"0")? {
        if rn_test(&mut rn, u, b"xX")? {
            hex = true;
        } else {
            count = 1;
        }
    }
    count += rn_digits(&mut rn, u, hex)?;
    if rn_test(&mut rn, u, b".")? {
        count += rn_digits(&mut rn, u, hex)?;
    }
    if count > 0 && rn_test(&mut rn, u, if hex { b"pP" } else { b"eE" })? {
        rn_test(&mut rn, u, b"-+")?;
        rn_digits(&mut rn, u, false)?;
    }
    if let Some(b) = rn.c {
        unget(u, &[b]);
    }
    Ok(rn.buf)
}

// ---- lines ----

/// Most read formats a line iterator may carry: 5.2 `LUA_MINSTACK - 3`,
/// 5.3+ `MAXARGLINE`. The argument number in the error is the limit's own
/// (5.2) or two past it (5.3+).
fn check_line_formats(vm: &mut Vm, n: u32) -> Result<(), LuaError> {
    let (max, argno, msg) = match vm.version() {
        LuaVersion::Lua51 => return Ok(()),
        LuaVersion::Lua52 => (17, 17, "too many options"),
        _ => (250, 252, "too many arguments"),
    };
    if n > max {
        return Err(arg_error(vm, argno, msg));
    }
    Ok(())
}

/// `aux_lines`: an iterator over `u` with upvalues [file, toclose, fmt...].
/// 5.1's iterator takes no formats.
fn make_lines(vm: &mut Vm, u: Gc<Userdata>, toclose: bool, fmts: &[Value]) -> Value {
    let mut up = vec![Value::Userdata(u), Value::Bool(toclose)];
    if vm.version() >= LuaVersion::Lua52 {
        up.extend_from_slice(fmts);
    }
    vm.native_with(io_readline, up.into_boxed_slice())
}

fn f_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    check_line_formats(vm, nargs.saturating_sub(1))?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let it = make_lines(vm, u, false, &fmts);
    Ok(vm.nat_return(fs, &[it]))
}

fn io_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // (5.1 checks index 1 after pushing the default input above it, so an
    // explicit nil fails there; luna treats nil as "no file name", as the
    // manual and every later version do.)
    let (u, toclose) = if a.is_none_or_nil(vm, 0) {
        let d = default_file(vm, Io::Input);
        if d.file().is_closed() {
            return Err(raise_str(vm, "attempt to use a closed file"));
        }
        (d, false)
    } else {
        let name = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
        (open_checked(vm, &name, b"r")?, true)
    };
    let nfmt = nargs.saturating_sub(1);
    check_line_formats(vm, nfmt)?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let it = make_lines(vm, u, toclose, &fmts);
    // 5.4+ return the file as the generic for's closing value
    if toclose && vm.version() >= LuaVersion::Lua54 {
        return Ok(vm.nat_return(fs, &[it, Value::Nil, Value::Nil, Value::Userdata(u)]));
    }
    Ok(vm.nat_return(fs, &[it]))
}

/// `io_readline`: one step of a line iterator.
fn io_readline(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let Value::Userdata(u) = vm.nat_upval(fs, 0) else {
        unreachable!("line iterator upvalue 0 is its file");
    };
    if u.file().is_closed() {
        return Err(raise_str(vm, "file is already closed"));
    }
    let fmts: Vec<Value> = (2..vm.nat_upcount(fs))
        .map(|i| vm.nat_upval(fs, i))
        .collect();
    let vals = match g_read(vm, u, &fmts, 2)? {
        ReadOut::Values(v) => v,
        // the read's error message is raised
        ReadOut::Error(e) => return Err(raise_str(vm, &strerror(&e))),
    };
    // ≤5.2 continue on a non-nil first value, 5.3+ on a true one; the only
    // false-ish value a read produces is nil, so the tests agree
    if !vals[0].is_nil() {
        return Ok(vm.nat_return(fs, &vals));
    }
    if let Value::Bool(true) = vm.nat_upval(fs, 1) {
        let _ = close_stream(u); // aux_close's results are dropped here too
    }
    Ok(vm.nat_return(fs, &[]))
}
