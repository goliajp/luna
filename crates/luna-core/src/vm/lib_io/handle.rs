//! `FILE*` handles: creation, argument checks, closing and the metamethods.

use super::*;

/// A new handle carrying the `FILE*` metatable. Like `luaL_setmetatable` on
/// a metatable with `__gc`, this registers it for finalization, so a file
/// nobody closed is still flushed and closed when collected or at state
/// close. `writable` gives it a user-space output buffer.
pub(super) fn new_file(vm: &mut Vm, fh: FileHandle, writable: bool) -> Gc<Userdata> {
    let u = vm.heap.new_userdata(UserdataPayload::File(fh), writable);
    // SAFETY: `u` was allocated on the line above and no other handle to it exists yet; the borrow covers one call
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
pub(super) fn check_open(vm: &mut Vm, a: Args, i: u32) -> Result<Gc<Userdata>, LuaError> {
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
pub(super) fn io_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
pub(super) fn f_close_51(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(vm, 1, "FILE* expected, got nil"));
    }
    f_close(vm, fs, nargs)
}

pub(super) fn f_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    let c = close_stream(u);
    Ok(push_closed(vm, fs, c))
}

/// `__gc` (and 5.4's `__close`): close an open handle, ignoring the outcome.
pub(super) fn f_gc(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_stream(vm, Args::new(fs, nargs), 0)?;
    if !u.file().is_closed() {
        let _ = close_stream(u); // PUC's f_gc drops aux_close's results
    }
    Ok(vm.nat_return(fs, &[]))
}

pub(super) fn f_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_stream(vm, Args::new(fs, nargs), 0)?;
    let s = if u.file().is_closed() {
        "file (closed)".to_string()
    } else {
        format!("file ({})", crate::vm::cfmt::c_pointer(u.as_ptr() as usize))
    };
    let v = Value::Str(vm.heap.intern(s.as_bytes()));
    Ok(vm.nat_return(fs, &[v]))
}

pub(super) fn io_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    let r = match test_file(vm, v) {
        None => Value::Nil,
        Some(u) if u.file().is_closed() => Value::Str(vm.heap.intern(b"closed file")),
        Some(_) => Value::Str(vm.heap.intern(b"file")),
    };
    Ok(vm.nat_return(fs, &[r]))
}
