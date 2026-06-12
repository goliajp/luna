//! Minimal base library — what the P03 gate corpus needs. The full base
//! library (P04) replaces/extends this.

use std::io::Write;

use crate::runtime::{Table, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_base(vm: &mut Vm) {
    let f = vm.native(nat_assert);
    vm.set_global("assert", f);
    let f = vm.native(nat_error);
    vm.set_global("error", f);
    let f = vm.native(nat_pcall);
    vm.set_global("pcall", f);
    let f = vm.native(nat_type);
    vm.set_global("type", f);
    let f = vm.native(nat_print);
    vm.set_global("print", f);
    let f = vm.native(nat_tostring);
    vm.set_global("tostring", f);
    let f = vm.native(nat_rawget);
    vm.set_global("rawget", f);
    let f = vm.native(nat_rawset);
    vm.set_global("rawset", f);
    let f = vm.native(nat_rawequal);
    vm.set_global("rawequal", f);
    let f = vm.native(nat_rawlen);
    vm.set_global("rawlen", f);
    let f = vm.native(nat_setmetatable);
    vm.set_global("setmetatable", f);
    let f = vm.native(nat_getmetatable);
    vm.set_global("getmetatable", f);
    let f = vm.native(nat_select);
    vm.set_global("select", f);
    // pairs returns the same object as the global next (PUC identity)
    let next_obj = vm.native(nat_next);
    vm.set_global("next", next_obj);
    let pairs_obj = vm.native_with(nat_pairs, Box::new([next_obj]));
    vm.set_global("pairs", pairs_obj);
    let ipairs_it = vm.native(ipairs_iter);
    let ipairs_obj = vm.native_with(nat_ipairs, Box::new([ipairs_it]));
    vm.set_global("ipairs", ipairs_obj);
    let f = vm.native(nat_tonumber);
    vm.set_global("tonumber", f);
    let f = vm.native(nat_load);
    vm.set_global("load", f);
    let f = vm.native(nat_collectgarbage);
    vm.set_global("collectgarbage", f);
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

pub(crate) fn check_table(
    vm: &mut Vm,
    v: Value,
    who: &str,
) -> Result<crate::runtime::Gc<Table>, LuaError> {
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

pub(crate) fn raise_str(vm: &mut Vm, msg: &str) -> LuaError {
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
    let it = vm.nat_upval(fs, 0);
    Ok(vm.nat_return(fs, &[it, t, Value::Nil]))
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
    let it = vm.nat_upval(fs, 0);
    Ok(vm.nat_return(fs, &[it, t, Value::Int(0)]))
}

// ---- shared helpers for the library modules ----

/// PUC luaL_argerror shape: bad argument #n to 'who' (extra).
pub(crate) fn arg_error(vm: &mut Vm, n: u32, who: &str, extra: &str) -> LuaError {
    raise_str(vm, &format!("bad argument #{n} to '{who}' ({extra})"))
}

pub(crate) fn nat_tonumber(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    if nargs < 2 || vm.nat_arg(fs, nargs, 1).is_nil() {
        let out = match v {
            Value::Int(_) | Value::Float(_) => v,
            Value::Str(s) => match crate::numeric::str2num(s.as_bytes(), true, true) {
                Some(crate::numeric::Num::Int(i)) => Value::Int(i),
                Some(crate::numeric::Num::Float(f)) => Value::Float(f),
                None => Value::Nil,
            },
            _ => Value::Nil,
        };
        return Ok(vm.nat_return(fs, &[out]));
    }
    let base = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as a base")?;
    if !(2..=36).contains(&base) {
        return Err(arg_error(vm, 2, "tonumber", "base out of range"));
    }
    let Value::Str(s) = v else {
        return Err(arg_error(
            vm,
            1,
            "tonumber",
            &format!("string expected, got {}", v.type_name()),
        ));
    };
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let neg = i < bytes.len() && bytes[i] == b'-';
    if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
        i += 1;
    }
    let digits_start = i;
    let mut acc: u64 = 0;
    while i < bytes.len() {
        let d = match bytes[i] {
            c @ b'0'..=b'9' => (c - b'0') as i64,
            c @ b'a'..=b'z' => (c - b'a' + 10) as i64,
            c @ b'A'..=b'Z' => (c - b'A' + 10) as i64,
            _ => break,
        };
        if d >= base {
            break;
        }
        acc = acc.wrapping_mul(base as u64).wrapping_add(d as u64);
        i += 1;
    }
    let had_digits = i > digits_start;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let out = if had_digits && i == bytes.len() {
        let v = if neg { acc.wrapping_neg() } else { acc };
        Value::Int(v as i64)
    } else {
        Value::Nil
    };
    Ok(vm.nat_return(fs, &[out]))
}

pub(crate) fn nat_load(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let chunk = vm.nat_arg(fs, nargs, 0);
    let Value::Str(src) = chunk else {
        // function-reader chunks arrive with the io work (P07)
        return Err(arg_error(
            vm,
            1,
            "load",
            &format!("string expected, got {}", chunk.type_name()),
        ));
    };
    let name: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => src.as_bytes().to_vec(), // PUC default: the chunk itself
    };
    let src_bytes = src.as_bytes().to_vec();
    match vm.load(&src_bytes, &name) {
        Ok(cl) => {
            if nargs >= 4 {
                let env = vm.nat_arg(fs, nargs, 3);
                let uv = vm.heap.new_upvalue(crate::runtime::UpvalState::Closed(env));
                unsafe { cl.as_mut() }.upvals[0] = uv;
            }
            Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
        }
        Err(e) => {
            let msg = format!("[string \"{}\"]:{}", String::from_utf8_lossy(&name), e);
            let m = Value::Str(vm.heap.intern(msg.as_bytes()));
            Ok(vm.nat_return(fs, &[Value::Nil, m]))
        }
    }
}

pub(crate) fn nat_collectgarbage(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let opt: Vec<u8> = match vm.nat_arg(fs, nargs, 0) {
        Value::Nil => b"collect".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                1,
                "collectgarbage",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let out = match opt.as_slice() {
        b"collect" => {
            vm.collect_garbage();
            Value::Int(0)
        }
        b"count" => Value::Float(vm.heap.bytes() as f64 / 1024.0),
        b"step" => {
            vm.collect_garbage();
            Value::Bool(true)
        }
        // pacing/mode options become meaningful with the P06 collector
        _ => Value::Int(0),
    };
    Ok(vm.nat_return(fs, &[out]))
}
