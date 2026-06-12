//! Minimal debug library — the introspection subset the official suite
//! leans on. Hooks are stubs until P05 (db.lua scope); getlocal/setlocal
//! need local-name debug info in Proto (P05).

use crate::runtime::{Gc, LuaClosure, Value};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_debug(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        unsafe { t.as_mut() }.set(k, fv).expect("valid key");
    };
    set(vm, "getinfo", d_getinfo);
    set(vm, "getupvalue", d_getupvalue);
    set(vm, "setupvalue", d_setupvalue);
    set(vm, "upvaluejoin", d_upvaluejoin);
    set(vm, "upvalueid", d_upvalueid);
    set(vm, "traceback", d_traceback);
    set(vm, "sethook", d_sethook);
    set(vm, "gethook", d_gethook);
    set(vm, "getlocal", d_getlocal);
    set(vm, "getmetatable", d_getmetatable);
    set(vm, "setmetatable", d_setmetatable);
    vm.set_global("debug", Value::Table(t));
    // register in package.loaded so require"debug" finds it
    let pkg_k = Value::Str(vm.heap.intern(b"package"));
    if let Value::Table(pkg) = vm.globals().get(pkg_k) {
        let lk = Value::Str(vm.heap.intern(b"loaded"));
        if let Value::Table(loaded) = pkg.get(lk) {
            let dk = Value::Str(vm.heap.intern(b"debug"));
            unsafe { loaded.as_mut() }
                .set(dk, Value::Table(t))
                .expect("valid key");
        }
    }
}

fn set_field(vm: &mut Vm, t: Gc<crate::runtime::Table>, k: &str, v: Value) {
    let key = Value::Str(vm.heap.intern(k.as_bytes()));
    unsafe { t.as_mut() }.set(key, v).expect("valid key");
}

fn info_for_closure(
    vm: &mut Vm,
    cl: Gc<LuaClosure>,
    out: Gc<crate::runtime::Table>,
    currentline: Option<u32>,
    extraargs: Option<i64>,
) {
    let proto = cl.proto;
    let src_bytes = unsafe { crate::runtime::string::bytes_of(proto.source.as_ptr()) }.to_vec();
    let display: Vec<u8> = match src_bytes.first() {
        Some(b'@') | Some(b'=') => src_bytes[1..].to_vec(),
        _ => {
            let mut d = b"[string \"".to_vec();
            d.extend(src_bytes.iter().take(40));
            d.extend_from_slice(b"\"]");
            d
        }
    };
    let source = Value::Str(vm.heap.intern(&src_bytes));
    set_field(vm, out, "source", source);
    let short = Value::Str(vm.heap.intern(&display));
    set_field(vm, out, "short_src", short);
    set_field(
        vm,
        out,
        "linedefined",
        Value::Int(proto.line_defined as i64),
    );
    set_field(
        vm,
        out,
        "lastlinedefined",
        Value::Int(*proto.lines.last().unwrap_or(&0) as i64),
    );
    let what = if proto.line_defined == 0 {
        "main"
    } else {
        "Lua"
    };
    let wv = Value::Str(vm.heap.intern(what.as_bytes()));
    set_field(vm, out, "what", wv);
    set_field(vm, out, "nups", Value::Int(cl.upvals.len() as i64));
    set_field(vm, out, "nparams", Value::Int(proto.num_params as i64));
    set_field(vm, out, "isvararg", Value::Bool(proto.is_vararg));
    set_field(vm, out, "func", Value::Closure(cl));
    set_field(
        vm,
        out,
        "currentline",
        currentline
            .map(|l| Value::Int(l as i64))
            .unwrap_or(Value::Int(-1)),
    );
    set_field(vm, out, "istailcall", Value::Bool(false));
    if let Some(e) = extraargs {
        set_field(vm, out, "extraargs", Value::Int(e));
    } else {
        set_field(vm, out, "extraargs", Value::Int(0));
    }
}

fn d_getinfo(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // (f | level [, what]) — thread argument not supported until coroutines
    let subject = vm.nat_arg(fs, nargs, 0);
    let out = vm.heap.new_table();
    match subject {
        Value::Closure(cl) => {
            info_for_closure(vm, cl, out, None, None);
        }
        Value::Native(_) => {
            let c = Value::Str(vm.heap.intern(b"C"));
            set_field(vm, out, "what", c);
            let src = Value::Str(vm.heap.intern(b"=[C]"));
            set_field(vm, out, "source", src);
            let short = Value::Str(vm.heap.intern(b"[C]"));
            set_field(vm, out, "short_src", short);
            set_field(vm, out, "currentline", Value::Int(-1));
            set_field(vm, out, "linedefined", Value::Int(-1));
            set_field(vm, out, "func", subject);
        }
        Value::Int(level) => {
            // level 1 = the Lua function that called getinfo
            let Some((cl, line, extraargs)) = vm.frame_info(level) else {
                return Ok(vm.nat_return(fs, &[Value::Nil]));
            };
            info_for_closure(vm, cl, out, Some(line), Some(extraargs));
        }
        v => {
            return Err(arg_error(
                vm,
                1,
                "getinfo",
                &format!("function or level expected, got {}", v.type_name()),
            ));
        }
    }
    Ok(vm.nat_return(fs, &[Value::Table(out)]))
}

fn check_closure(vm: &mut Vm, v: Value, who: &str) -> Result<Gc<LuaClosure>, LuaError> {
    match v {
        Value::Closure(cl) => Ok(cl),
        v => Err(arg_error(
            vm,
            1,
            who,
            &format!("function expected, got {}", v.type_name()),
        )),
    }
}

fn d_getupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    if let Value::Native(_) = f {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    let cl = check_closure(vm, f, "getupvalue")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    if n < 1 || n as usize > cl.upvals.len() {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    let idx = (n - 1) as usize;
    let name = cl.proto.upvals[idx].name.clone();
    let value = vm.upvalue_value(cl, idx);
    let nv = Value::Str(vm.heap.intern(name.as_bytes()));
    Ok(vm.nat_return(fs, &[nv, value]))
}

fn d_setupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    let cl = check_closure(vm, f, "setupvalue")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let v = vm.nat_arg(fs, nargs, 2);
    if n < 1 || n as usize > cl.upvals.len() {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    let idx = (n - 1) as usize;
    vm.upvalue_set_value(cl, idx, v);
    let name = cl.proto.upvals[idx].name.clone();
    let nv = Value::Str(vm.heap.intern(name.as_bytes()));
    Ok(vm.nat_return(fs, &[nv]))
}

fn d_upvaluejoin(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f1 = check_closure(vm, vm.nat_arg(fs, nargs, 0), "upvaluejoin")?;
    let n1 = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let f2 = check_closure(vm, vm.nat_arg(fs, nargs, 2), "upvaluejoin")?;
    let n2 = vm.int_from(vm.nat_arg(fs, nargs, 3), "use as an index")?;
    if n1 < 1 || n1 as usize > f1.upvals.len() || n2 < 1 || n2 as usize > f2.upvals.len() {
        return Err(raise_str(vm, "bad upvalue index to 'upvaluejoin'"));
    }
    let uv = f2.upvals[(n2 - 1) as usize];
    unsafe { f1.as_mut() }.upvals[(n1 - 1) as usize] = uv;
    Ok(0)
}

fn d_upvalueid(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = check_closure(vm, vm.nat_arg(fs, nargs, 0), "upvalueid")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    if n < 1 || n as usize > f.upvals.len() {
        return Err(raise_str(vm, "bad upvalue index to 'upvalueid'"));
    }
    // identity token (PUC returns a light userdata; an integer compares the
    // same way until userdata exists)
    let id = f.upvals[(n - 1) as usize].as_ptr() as i64;
    Ok(vm.nat_return(fs, &[Value::Int(id)]))
}

fn d_traceback(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let msg = match vm.nat_arg(fs, nargs, 0) {
        Value::Nil => Vec::new(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            // non-string messages are returned unchanged (PUC)
            return Ok(vm.nat_return(fs, &[v]));
        }
    };
    let mut out = msg;
    out.extend_from_slice(b"\nstack traceback:");
    out.extend(vm.traceback_bytes());
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn d_sethook(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    // hooks land in P05 (db.lua)
    Ok(0)
}

fn d_gethook(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

fn d_getlocal(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    // needs local-name debug info in Proto (P05)
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

fn d_getmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let mt = vm.metatable_of(v).map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[mt]))
}

fn d_setmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let mt = vm.nat_arg(fs, nargs, 1);
    if let Value::Table(t) = v {
        match mt {
            Value::Nil => unsafe { t.as_mut() }.set_metatable(None),
            Value::Table(m) => unsafe { t.as_mut() }.set_metatable(Some(m)),
            _ => return Err(arg_error(vm, 2, "setmetatable", "nil or table expected")),
        }
    }
    Ok(vm.nat_return(fs, &[v]))
}
