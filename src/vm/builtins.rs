//! Minimal base library — what the P03 gate corpus needs. The full base
//! library (P04) replaces/extends this.

use std::io::Write;

use crate::runtime::{Table, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_base(vm: &mut Vm) {
    vm.set_global("assert", Value::Native(nat_assert));
    vm.set_global("error", Value::Native(nat_error));
    vm.set_global("pcall", Value::Native(nat_pcall));
    vm.set_global("type", Value::Native(nat_type));
    vm.set_global("print", Value::Native(nat_print));
    vm.set_global("tostring", Value::Native(nat_tostring));
    vm.set_global("rawget", Value::Native(nat_rawget));
    vm.set_global("rawset", Value::Native(nat_rawset));
    vm.set_global("rawequal", Value::Native(nat_rawequal));
    vm.set_global("rawlen", Value::Native(nat_rawlen));
    vm.set_global("setmetatable", Value::Native(nat_setmetatable));
    vm.set_global("getmetatable", Value::Native(nat_getmetatable));
    vm.set_global("select", Value::Native(nat_select));
    vm.set_global("next", Value::Native(nat_next));
    vm.set_global("pairs", Value::Native(nat_pairs));
    vm.set_global("ipairs", Value::Native(nat_ipairs));
    let version = match vm.version() {
        crate::version::LuaVersion::Lua51 => "Lua 5.1",
        crate::version::LuaVersion::Lua54 => "Lua 5.4",
        crate::version::LuaVersion::Lua55 => "Lua 5.5",
    };
    let v = Value::Str(vm.heap.intern(version.as_bytes()));
    vm.set_global("_VERSION", v);
    let g = Value::Table(vm.globals());
    vm.set_global("_G", g);
}

fn check_table(vm: &mut Vm, v: Value, who: &str) -> Result<crate::runtime::Gc<Table>, LuaError> {
    match v {
        Value::Table(t) => Ok(t),
        v => Err(vm.rt_err(&format!(
            "bad argument to '{who}' (table expected, got {})",
            v.type_name()
        ))),
    }
}

fn nat_assert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    if v.truthy() {
        // assert returns all its arguments
        let vals: Vec<Value> = (0..nargs.max(1))
            .map(|i| vm.nat_arg(fs, nargs, i))
            .collect();
        return Ok(vm.nat_return(fs, &vals));
    }
    if nargs >= 2 {
        let msg = vm.nat_arg(fs, nargs, 1);
        return Err(raise(vm, msg));
    }
    Err(raise_str(vm, "assertion failed!"))
}

fn nat_error(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let msg = vm.nat_arg(fs, nargs, 0);
    let level = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => 1,
        v => vm.int_from(v, "use as a level")?,
    };
    if level > 0 {
        return Err(raise(vm, msg));
    }
    // level 0: no position information added
    Err(LuaError(msg))
}

/// Raise a string-ish error with the caller's position prefix (PUC level 1).
fn raise(vm: &mut Vm, msg: Value) -> LuaError {
    match msg {
        Value::Str(s) => {
            let text = match vm.position_prefix() {
                Some(p) => {
                    let mut t = p.into_bytes();
                    t.extend_from_slice(s.as_bytes());
                    t
                }
                None => s.as_bytes().to_vec(),
            };
            LuaError(Value::Str(vm.heap.intern(&text)))
        }
        v => LuaError(v),
    }
}

fn raise_str(vm: &mut Vm, msg: &str) -> LuaError {
    let s = Value::Str(vm.heap.intern(msg.as_bytes()));
    raise(vm, s)
}

fn nat_pcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(raise_str(vm, "bad argument #1 to 'pcall' (value expected)"));
    }
    let f = vm.nat_arg(fs, nargs, 0);
    let args: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    match vm.call_value(f, &args) {
        Ok(results) => {
            let mut out = Vec::with_capacity(results.len() + 1);
            out.push(Value::Bool(true));
            out.extend(results);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => Ok(vm.nat_return(fs, &[Value::Bool(false), e.0])),
    }
}

fn nat_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(raise_str(vm, "bad argument #1 to 'type' (value expected)"));
    }
    let v = vm.nat_arg(fs, nargs, 0);
    let s = Value::Str(vm.heap.intern(v.type_name().as_bytes()));
    Ok(vm.nat_return(fs, &[s]))
}

fn nat_print(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let mut out = Vec::new();
    for i in 0..nargs {
        if i > 0 {
            out.push(b'\t');
        }
        let v = vm.nat_arg(fs, nargs, i);
        out.extend(vm.tostring_value(v)?);
    }
    out.push(b'\n');
    let _ = std::io::stdout().write_all(&out);
    Ok(0)
}

fn nat_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let bytes = vm.tostring_value(v)?;
    let s = Value::Str(vm.heap.intern(&bytes));
    Ok(vm.nat_return(fs, &[s]))
}

fn nat_rawget(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "rawget")?;
    let k = vm.nat_arg(fs, nargs, 1);
    let v = t.get(k);
    Ok(vm.nat_return(fs, &[v]))
}

fn nat_rawset(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, tv, "rawset")?;
    let k = vm.nat_arg(fs, nargs, 1);
    let v = vm.nat_arg(fs, nargs, 2);
    match unsafe { t.as_mut() }.set(k, v) {
        Ok(()) => Ok(vm.nat_return(fs, &[tv])),
        Err(crate::runtime::TableError::NilIndex) => Err(raise_str(vm, "table index is nil")),
        Err(crate::runtime::TableError::NanIndex) => Err(raise_str(vm, "table index is NaN")),
        Err(_) => unreachable!(),
    }
}

fn nat_rawequal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = vm.nat_arg(fs, nargs, 0);
    let b = vm.nat_arg(fs, nargs, 1);
    Ok(vm.nat_return(fs, &[Value::Bool(a.raw_eq(b))]))
}

fn nat_rawlen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let n = match v {
        Value::Table(t) => t.len(),
        Value::Str(s) => s.len() as i64,
        _ => return Err(raise_str(vm, "table or string expected")),
    };
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

fn nat_setmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let tv = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, tv, "setmetatable")?;
    if !vm.get_mm(tv, Mm::Metatable).is_nil() {
        return Err(raise_str(vm, "cannot change a protected metatable"));
    }
    let mt = vm.nat_arg(fs, nargs, 1);
    match mt {
        Value::Nil => unsafe { t.as_mut() }.set_metatable(None),
        Value::Table(m) => unsafe { t.as_mut() }.set_metatable(Some(m)),
        _ => return Err(raise_str(vm, "nil or table expected")),
    }
    Ok(vm.nat_return(fs, &[tv]))
}

fn nat_getmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let v = vm.nat_arg(fs, nargs, 0);
    // __metatable protection: return that field instead
    let protected = vm.get_mm(v, Mm::Metatable);
    if !protected.is_nil() {
        return Ok(vm.nat_return(fs, &[protected]));
    }
    let mt = vm.metatable_of(v).map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[mt]))
}

fn nat_select(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let sel = vm.nat_arg(fs, nargs, 0);
    if let Value::Str(s) = sel
        && s.as_bytes() == b"#"
    {
        return Ok(vm.nat_return(fs, &[Value::Int(nargs as i64 - 1)]));
    }
    let n = vm.int_from(sel, "use as an index")?;
    let extras = nargs as i64 - 1;
    let start = if n > 0 {
        n
    } else if n < 0 {
        let s = extras + n + 1;
        if s < 1 {
            return Err(raise_str(
                vm,
                "bad argument #1 to 'select' (index out of range)",
            ));
        }
        s
    } else {
        return Err(raise_str(
            vm,
            "bad argument #1 to 'select' (index out of range)",
        ));
    };
    let vals: Vec<Value> = (start..=extras)
        .map(|i| vm.nat_arg(fs, nargs, i as u32))
        .collect();
    Ok(vm.nat_return(fs, &vals))
}

fn nat_next(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "next")?;
    let k = vm.nat_arg(fs, nargs, 1);
    match t.next(k) {
        Ok(Some((k, v))) => Ok(vm.nat_return(fs, &[k, v])),
        Ok(None) => Ok(vm.nat_return(fs, &[Value::Nil])),
        Err(_) => Err(raise_str(vm, "invalid key to 'next'")),
    }
}

fn nat_pairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    check_table(vm, t, "pairs")?;
    Ok(vm.nat_return(fs, &[Value::Native(nat_next), t, Value::Nil]))
}

fn ipairs_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "ipairs")?;
    let i = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(i) => i,
        v => vm.int_from(v, "use as an index")?,
    };
    let next_i = i + 1;
    let v = t.get_int(next_i);
    if v.is_nil() {
        Ok(vm.nat_return(fs, &[Value::Nil]))
    } else {
        Ok(vm.nat_return(fs, &[Value::Int(next_i), v]))
    }
}

fn nat_ipairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    check_table(vm, t, "ipairs")?;
    Ok(vm.nat_return(fs, &[Value::Native(ipairs_iter), t, Value::Int(0)]))
}
