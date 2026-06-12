//! Minimal os/io surface for the `_U` (user) official test mode, plus
//! loadfile/dofile. The full io model (files, handles) is P07.

use std::io::Write;

use crate::runtime::Value;
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_os_io(vm: &mut Vm) {
    let os = vm.heap.new_table();
    let set = |vm: &mut Vm, t: crate::runtime::Gc<crate::runtime::Table>, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        unsafe { t.as_mut() }.set(k, fv).expect("valid key");
    };
    set(vm, os, "time", os_time);
    set(vm, os, "clock", os_clock);
    set(vm, os, "date", os_date);
    set(vm, os, "getenv", os_getenv);
    vm.set_global("os", Value::Table(os));

    let io = vm.heap.new_table();
    set(vm, io, "write", io_write);
    set(vm, io, "read", io_read);
    vm.set_global("io", Value::Table(io));

    let f = vm.native(nat_loadfile);
    vm.set_global("loadfile", f);
    let f = vm.native(nat_dofile);
    vm.set_global("dofile", f);
}

fn os_time(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs > 0 && !vm.nat_arg(fs, nargs, 0).is_nil() {
        // table-based mktime arrives with the full os lib (P07)
        return Err(arg_error(vm, 1, "time", "table form not supported yet"));
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(vm.nat_return(fs, &[Value::Int(t)]))
}

fn os_clock(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let secs = vm.uptime().as_secs_f64();
    Ok(vm.nat_return(fs, &[Value::Float(secs)]))
}

fn os_date(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // minimal: fixed readable form; strftime formats are P07
    let _ = nargs;
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = format!("(date: {t} epoch seconds)");
    let v = Value::Str(vm.heap.intern(s.as_bytes()));
    Ok(vm.nat_return(fs, &[v]))
}

fn os_getenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "getenv", "string expected"));
    };
    let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
    let v = match std::env::var_os(&name) {
        Some(val) => Value::Str(vm.heap.intern(val.to_string_lossy().as_bytes())),
        None => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn io_write(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let mut out = Vec::new();
    for i in 0..nargs {
        match vm.nat_arg(fs, nargs, i) {
            Value::Str(s) => out.extend_from_slice(s.as_bytes()),
            v @ (Value::Int(_) | Value::Float(_)) => out.extend(vm.tostring_basic(v)),
            v => {
                return Err(arg_error(
                    vm,
                    i + 1,
                    "write",
                    &format!("string expected, got {}", v.type_name()),
                ));
            }
        }
    }
    let _ = std::io::stdout().write_all(&out);
    Ok(0)
}

fn io_read(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    // no interactive stdin in the test harness
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

fn load_path(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Result<Value, Value>, LuaError> {
    let Value::Str(path) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "loadfile", "string expected"));
    };
    let path_s = String::from_utf8_lossy(path.as_bytes()).into_owned();
    match std::fs::read(&path_s) {
        Ok(src) => {
            let mut chunkname = vec![b'@'];
            chunkname.extend_from_slice(path.as_bytes());
            match vm.load(&src, &chunkname) {
                Ok(cl) => Ok(Ok(Value::Closure(cl))),
                Err(e) => {
                    let msg = format!("{path_s}:{e}");
                    Ok(Err(Value::Str(vm.heap.intern(msg.as_bytes()))))
                }
            }
        }
        Err(e) => {
            let msg = format!("cannot open {path_s} ({e})");
            Ok(Err(Value::Str(vm.heap.intern(msg.as_bytes()))))
        }
    }
}

fn nat_loadfile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    match load_path(vm, fs, nargs)? {
        Ok(f) => Ok(vm.nat_return(fs, &[f])),
        Err(msg) => Ok(vm.nat_return(fs, &[Value::Nil, msg])),
    }
}

fn nat_dofile(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    match load_path(vm, fs, nargs)? {
        Ok(f) => {
            let results = vm.call_value(f, &[])?;
            Ok(vm.nat_return(fs, &results))
        }
        Err(msg) => Err(raise_str(vm, &vm_text(vm, msg))),
    }
}

fn vm_text(_vm: &Vm, v: Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        _ => format!("(error object is a {} value)", v.type_name()),
    }
}

// ---- minimal package / require ----

pub(crate) fn open_package(vm: &mut Vm) {
    let pkg = vm.heap.new_table();
    let loaded = vm.heap.new_table();
    // prepopulate with the standard libraries (PUC does the same)
    for name in [
        "string", "math", "table", "os", "io", "utf8", "debug", "_G", "package",
    ] {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        let v = if name == "package" {
            Value::Table(pkg)
        } else if name == "_G" {
            Value::Table(vm.globals())
        } else {
            let gk = Value::Str(vm.heap.intern(name.as_bytes()));
            vm.globals().get(gk)
        };
        unsafe { loaded.as_mut() }.set(k, v).expect("valid key");
    }
    let lk = Value::Str(vm.heap.intern(b"loaded"));
    unsafe { pkg.as_mut() }
        .set(lk, Value::Table(loaded))
        .expect("valid key");
    let pk = Value::Str(vm.heap.intern(b"path"));
    let pv = Value::Str(vm.heap.intern(b"./?.lua"));
    unsafe { pkg.as_mut() }.set(pk, pv).expect("valid key");
    vm.set_global("package", Value::Table(pkg));
    let req = vm.native_with(nat_require, Box::new([Value::Table(loaded)]));
    vm.set_global("require", req);
}

fn nat_require(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "require", "string expected"));
    };
    let Value::Table(loaded) = vm.nat_upval(fs, 0) else {
        unreachable!()
    };
    let key = Value::Str(name);
    let cached = loaded.get(key);
    if !cached.is_nil() {
        return Ok(vm.nat_return(fs, &[cached]));
    }
    // file searcher: ./<name>.lua (P07 brings full package.path semantics)
    let name_s = String::from_utf8_lossy(name.as_bytes()).into_owned();
    let path = format!("./{name_s}.lua");
    let Ok(src) = std::fs::read(&path) else {
        return Err(raise_str(
            vm,
            &format!("module '{name_s}' not found:\n\tno file '{path}'"),
        ));
    };
    let chunkname = format!("@{path}");
    let cl = match vm.load(&src, chunkname.as_bytes()) {
        Ok(cl) => cl,
        Err(e) => {
            return Err(raise_str(
                vm,
                &format!("error loading module '{name_s}': {e}"),
            ));
        }
    };
    let pv = Value::Str(vm.heap.intern(path.as_bytes()));
    let results = vm.call_value(Value::Closure(cl), &[key, pv])?;
    let value = results.first().copied().unwrap_or(Value::Nil);
    let value = if value.is_nil() {
        Value::Bool(true)
    } else {
        value
    };
    unsafe { loaded.as_mut() }
        .set(key, value)
        .expect("valid key");
    Ok(vm.nat_return(fs, &[value, pv]))
}
