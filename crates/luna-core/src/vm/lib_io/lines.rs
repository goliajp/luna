//! Line iterators: `io.lines` and `file:lines`.

use super::*;
use crate::runtime::puc::api::PucCollector;

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

/// `aux_lines`: an iterator over `u` with upvalues [file, toclose, fmt...],
/// made as PUC makes it (`lua_pushcclosure`), with the number of formats
/// as one more upvalue from 5.2 on. 5.1's iterator takes no formats.
fn make_lines(
    vm: &mut Vm,
    u: Gc<Userdata>,
    toclose: bool,
    fmts: &[Value],
) -> Result<Value, LuaError> {
    let mut up = crate::runtime::mem::LVec::new(vm.heap.mem());
    up.extend_from_slice(&[Value::Userdata(u), Value::Bool(toclose)])?;
    let mut nup = 2;
    if vm.version() >= LuaVersion::Lua52 {
        up.extend_from_slice(fmts)?;
        nup = 3 + fmts.len();
    }
    vm.lib_native_puc(io_readline, &up, nup)
}

pub(super) fn f_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    check_line_formats(vm, nargs.saturating_sub(1))?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let it = make_lines(vm, u, false, &fmts)?;
    vm.nat_ret(fs, &[it])
}

pub(super) fn io_lines(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // (5.1 checks index 1 after pushing the default input above it, so an
    // explicit nil fails there; luna treats nil as "no file name", as the
    // manual and every later version do.)
    let (u, toclose) = if a.is_none_or_nil(vm, 0) {
        let d = default_file(vm, Io::Input);
        // the default input is fetched from the registry before the check
        vm.native_push(1);
        match d {
            Some(d) if !d.file().is_closed() => (d, false),
            _ => return Err(raise_str(vm, "attempt to use a closed file")),
        }
    } else {
        let name = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
        (open_checked(vm, &name, b"r")?, true)
    };
    let nfmt = nargs.saturating_sub(1);
    check_line_formats(vm, nfmt)?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let it = make_lines(vm, u, toclose, &fmts)?;
    // 5.1: made by `io.lines`, in its environment
    if let Value::Native(n) = it
        && vm.version() == LuaVersion::Lua51
    {
        vm.heap
            .set_env51(n.hdr(), crate::runtime::puc::records::env51::ENV_IO);
    }
    // 5.4+ return the file as the generic for's closing value
    if toclose && vm.version() >= LuaVersion::Lua54 {
        return vm.nat_ret(fs, &[it, Value::Nil, Value::Nil, Value::Userdata(u)]);
    }
    vm.nat_ret(fs, &[it])
}

/// `io_readline`: one step of a line iterator.
fn io_readline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Userdata(u) = vm.nat_upval(fs, 0) else {
        unreachable!("line iterator upvalue 0 is its file");
    };
    if u.file().is_closed() {
        return Err(raise_str(vm, "file is already closed"));
    }
    let fmts: Vec<Value> = (2..vm.nat_upcount(fs))
        .map(|i| vm.nat_upval(fs, i))
        .collect();
    // `lua_settop(L, 1)`, then the formats are pushed for `g_read` (5.2+)
    let mut top = fs + 1 + nargs;
    if vm.version() >= LuaVersion::Lua52 {
        vm.native_settop(1);
        top = fs + 2;
        if vm.version() >= LuaVersion::Lua53 {
            let n = fmts.len() as i64;
            crate::vm::argcheck::check_stack_at(vm, top, n, "too many arguments")?;
        }
        vm.native_push(fmts.len() as u32);
        top += fmts.len() as u32;
    }
    let vals = match g_read(vm, u, &fmts, 2, top)? {
        ReadOut::Values(v) => v,
        // the read's error message is raised
        ReadOut::Error(e) => return Err(readline_error(vm, &e)),
    };
    // ≤5.2 continue on a non-nil first value, 5.3+ on a true one; the only
    // false-ish value a read produces is nil, so the tests agree
    if !vals[0].is_nil() {
        return vm.nat_ret(fs, &vals);
    }
    if let Value::Bool(true) = vm.nat_upval(fs, 1) {
        let _ = close_stream(u); // aux_close's results are dropped here too
    }
    vm.nat_ret(fs, &[])
}

/// The error a line iterator's read failed with, raised: the line read so
/// far is pushed, then 5.2+ `luaL_fileresult`'s nil, message and errno.
fn readline_error(vm: &mut Vm, e: &std::io::Error) -> LuaError {
    note_failure(e);
    vm.native_push(if vm.version() >= LuaVersion::Lua52 {
        4
    } else {
        1
    });
    let msg = strerror(e);
    let args = [crate::runtime::puc::fmt::FmtArg::S(msg.as_bytes())];
    crate::vm::builtins::raise_lib(vm, b"%s", &args, 1)
}
