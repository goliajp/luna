//! `FILE*` handles: creation, argument checks, closing and the metamethods.

use super::*;

/// A file handle made as PUC's `newprefile` makes it: the userdata
/// (`lua_newuserdata`, which checks for a collection: `checked` makes the
/// check, the userdata held), then its metatable (`luaL_setmetatable`,
/// which marks it for finalization).
pub(super) fn new_file(
    vm: &mut Vm,
    fh: FileHandle,
    writable: bool,
    checked: impl FnOnce(&mut Vm, Gc<Userdata>) -> Result<(), LuaError>,
) -> Result<Gc<Userdata>, LuaError> {
    // PUC's handle: 5.1 a `FILE *`, later a `luaL_Stream` (the file and
    // its close function), from 5.4 with no user value
    let sz = vm.heap.sizes;
    let size = match vm.version() {
        LuaVersion::Lua51 => sz.userdata(1, sz.ptr),
        LuaVersion::Lua52 | LuaVersion::Lua53 => sz.userdata(1, 2 * sz.ptr),
        _ => sz.userdata(0, 2 * sz.ptr),
    };
    vm.api_check_before()?;
    let u = vm.lib_userdata(UserdataPayload::File(fh), writable, size)?;
    // 5.1: the environment of the io function that opens it
    if vm.version() == LuaVersion::Lua51 {
        vm.heap
            .set_env51(u.as_ptr() as *mut _, crate::runtime::heap::ENV_IO);
    }
    checked(vm, u)?;
    // SAFETY: `u` was allocated above and is held by the caller's check;
    // the borrow covers one call
    unsafe { u.as_mut() }.set_metatable(super::refs::file_metatable(vm));
    vm.retry_oom(|vm| vm.heap.register_finalizable_userdata(u))?;
    Ok(u)
}

/// `then`, run with the handle PUC makes before an open, `tmpfile` or
/// `popen` that fails (`newfile`): closed, held while `then` reports the
/// failure, garbage after.
pub(super) fn failed_handle<R>(
    vm: &mut Vm,
    then: impl FnOnce(&mut Vm) -> Result<R, LuaError>,
) -> Result<R, LuaError> {
    let u = new_file(vm, FileHandle::Closed, false, |vm, u| {
        vm.api_check_after(Value::Userdata(u))
    })?;
    vm.rooted(&[Value::Userdata(u)], |vm| then(vm))
}

/// `luaL_testudata(L, i, LUA_FILEHANDLE)`: a userdata whose metatable is the
/// `FILE*` one (luna only gives that metatable to file payloads, but
/// `debug.setmetatable` can hand it to any userdata).
fn test_file(vm: &Vm, v: Value) -> Option<Gc<Userdata>> {
    match v {
        Value::Userdata(u)
            if matches!(u.payload, UserdataPayload::File(_))
                && u.metatable()
                    .zip(super::refs::file_metatable(vm))
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
pub(super) fn check_open(vm: &mut Vm, a: Args, i: u32) -> Result<Gc<Userdata>, LuaError> {
    let u = check_stream(vm, a, i)?;
    if u.file().is_closed() {
        return Err(raise_str(vm, "attempt to use a closed file"));
    }
    Ok(u)
}

/// Drain every open handle's output buffer and stdout (C `fflush(NULL)`).
pub(crate) fn flush_all(vm: &mut Vm) {
    flush_these(vm.heap.finalizable_userdata());
}

/// [`flush_all`] as the state closes: every stream still open, a finalizer
/// or not (C's `exit` flushes them all).
pub(crate) fn flush_all_closing(vm: &mut Vm) {
    flush_these(vm.heap.all_userdata());
}

/// A stream the collector frees without closing it (its `__gc` taken
/// away): C's `FILE` stays until `exit` flushes it, so what it holds is
/// written out now.
pub(crate) fn write_out_freed(u: Gc<Userdata>) {
    if matches!(u.payload, UserdataPayload::File(ref fh) if !fh.is_closed()) {
        // as `exit`'s flush, a failure reports nothing
        let _ = drain_write_buf(u);
    }
}

fn flush_these(us: Vec<Gc<Userdata>>) {
    for u in us {
        if matches!(u.payload, UserdataPayload::File(ref fh) if !fh.is_closed()) {
            // like fflush(NULL), a stream that fails to flush does not stop
            // the others and reports nothing
            let _ = drain_write_buf(u);
        }
    }
    // same: fflush(NULL) reports nothing
    let _ = crate::stdio::flush_stdout();
    let _ = crate::stdio::flush_stderr();
}

// ---- closing ----

/// What closing a stream did (`aux_close` → `io_noclose` / `io_fclose` /
/// `io_pclose`).
pub(super) enum Closed {
    /// a standard stream, left open
    Std,
    /// a regular file: the flush-on-close result
    File(std::io::Result<()>),
    /// a popen stream: the wait result
    #[cfg(any(unix, windows))]
    Pipe(std::io::Result<std::process::ExitStatus>),
}

pub(super) fn close_stream(u: Gc<Userdata>) -> Closed {
    if u.file().is_std() {
        return Closed::Std;
    }
    let flushed = if u.crt.is_some() {
        crt::fclose(u)
    } else {
        drain_write_buf(u)
    };
    // SAFETY: `u` is a file handle the caller holds (a native argument on the stack, a registered finalizable, or the io library's default stream); `drain_write_buf`'s borrow has ended, and `m` is the only reference into it until return
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

fn push_closed(vm: &mut Vm, fs: u32, c: Closed) -> Result<u32, LuaError> {
    match c {
        Closed::Std => {
            let m = vm.str_value(b"cannot close standard file")?;
            vm.nat_ret(fs, &[Value::Nil, m])
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

/// `io_fclose` and `io_pclose` begin with `errno = 0` (5.4+); closing a
/// standard file (`io_noclose`) does not.
fn close_reset(vm: &Vm, u: Gc<Userdata>) {
    if matches!(u.file(), FileHandle::File(_)) {
        reset_errno(vm);
    }
}

/// `io.close([file])`, also the 5.2 method: no argument means the default
/// output.
pub(super) fn io_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = if nargs == 0 {
        let d = default_file(vm, Io::Output);
        vm.native_push(1);
        match d {
            Some(d) if !d.file().is_closed() => d,
            _ => return Err(raise_str(vm, "attempt to use a closed file")),
        }
    } else {
        check_open(vm, Args::new(fs, nargs), 0)?
    };
    close_reset(vm, u);
    let c = close_stream(u);
    push_closed(vm, fs, c)
}

/// 5.1's method `close` is `io_close` with an environment lacking the
/// default output, so without an argument it checks a nil.
pub(super) fn f_close_51(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(vm, 1, "FILE* expected, got nil"));
    }
    f_close(vm, fs, nargs)
}

pub(super) fn f_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    close_reset(vm, u);
    let c = close_stream(u);
    push_closed(vm, fs, c)
}

/// `__gc` (and 5.4's `__close`): close an open handle, ignoring the outcome.
pub(super) fn f_gc(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_stream(vm, Args::new(fs, nargs), 0)?;
    if !u.file().is_closed() {
        close_reset(vm, u);
        // PUC's f_gc drops aux_close's results; `io_noclose` makes its
        // message all the same
        if let Closed::Std = close_stream(u) {
            vm.api_push_string(b"cannot close standard file")?;
        }
    }
    vm.nat_ret(fs, &[])
}

pub(super) fn f_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_stream(vm, Args::new(fs, nargs), 0)?;
    let s = if u.file().is_closed() {
        "file (closed)".to_string()
    } else {
        format!("file ({})", crate::vm::cfmt::c_pointer(u.as_ptr() as usize))
    };
    let v = vm.api_push_string(s.as_bytes())?;
    vm.nat_ret(fs, &[v])
}

pub(super) fn io_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    let r = match test_file(vm, v) {
        None => Value::Nil,
        Some(u) if u.file().is_closed() => vm.api_push_string(b"closed file")?,
        Some(_) => vm.api_push_string(b"file")?,
    };
    vm.nat_ret(fs, &[r])
}
