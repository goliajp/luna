//! Line iterators: `io.lines` and `file:lines`.

use super::*;

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

pub(super) fn f_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    check_line_formats(vm, nargs.saturating_sub(1))?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let it = make_lines(vm, u, false, &fmts);
    Ok(vm.nat_return(fs, &[it]))
}

pub(super) fn io_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
        ReadOut::Error(e) => {
            note_failure(&e);
            return Err(raise_str(vm, &strerror(&e)));
        }
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
