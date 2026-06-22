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
    let f = vm.native(nat_xpcall);
    vm.set_global("xpcall", f);
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
    let load_obj = vm.native(nat_load);
    vm.set_global("load", load_obj);
    let f = vm.native(nat_collectgarbage);
    vm.set_global("collectgarbage", f);
    // PUC 5.4 introduced the warning system. `warn(msg1, …, msgN)` emits
    // pieces of one message via the default warnf (`lauxlib.c::warnfon/off`),
    // which recognises `@on` / `@off` control messages and starts disabled.
    if vm.version() >= crate::version::LuaVersion::Lua54 {
        let f = vm.native(nat_warn);
        vm.set_global("warn", f);
    }
    // PUC 5.1 globals retired in 5.2 (`unpack` → `table.unpack`) and 5.2
    // (`loadstring` → `load`). Provide aliases so the 5.1 test suite, which
    // is full of `unpack(...)` and `loadstring("...")` calls, still resolves.
    if vm.version() == crate::version::LuaVersion::Lua51 {
        vm.set_global("loadstring", load_obj);
        let f = vm.native(crate::vm::lib_table::t_unpack);
        vm.set_global("unpack", f);
        // PUC 5.1 also exposed `gcinfo()` (memory in KB) and `newproxy()`
        // (debug-table proxy with `__gc`). gcinfo is a thin wrapper around
        // `collectgarbage("count")`; newproxy is left in the backlog —
        // its `__gc` finalizer integration is non-trivial.
        let f = vm.native(nat_gcinfo);
        vm.set_global("gcinfo", f);
        // PUC 5.1 `setfenv`/`getfenv` — every Lua function carries its own
        // env (5.1 `LClosure.env`); 5.2 retired them in favour of the `_ENV`
        // upvalue model. The Op::Closure path here clones cell 0 per
        // closure under 5.1, so writing through the per-closure cell only
        // affects that closure (events.lua / locals.lua / nextvar.lua).
        let f = vm.native(nat_setfenv);
        vm.set_global("setfenv", f);
        let f = vm.native(nat_getfenv);
        vm.set_global("getfenv", f);
        let f = vm.native(nat_newproxy);
        vm.set_global("newproxy", f);
    }
    let version = match vm.version() {
        crate::version::LuaVersion::Lua51 => "Lua 5.1",
        crate::version::LuaVersion::Lua52 => "Lua 5.2",
        crate::version::LuaVersion::Lua53 => "Lua 5.3",
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
    check_table_at(vm, v, 1, who)
}

/// Variant that names the argument index — used by `table.move` whose second
/// table is at #5. Routes through `arg_error` so the running function's name
/// is qualified (e.g. `'sort'` → `'table.sort'`) when called from a nested
/// native (PUC pushglobalfuncname).
pub(crate) fn check_table_at(
    vm: &mut Vm,
    v: Value,
    n: u32,
    who: &str,
) -> Result<crate::runtime::Gc<Table>, LuaError> {
    match v {
        Value::Table(t) => Ok(t),
        v => {
            let got = vm.obj_typename(v);
            Err(arg_error(vm, n, who, &format!("table expected, got {got}")))
        }
    }
}

fn nat_assert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC `luaB_assert` calls `luaL_checkany(L, 1)` before inspecting the
    // condition — so `assert()` (with no arguments) raises the canonical
    // "bad argument #1 to 'assert' (value expected)" rather than the plain
    // "assertion failed!" string. errors.lua :672 looks for "value expected".
    if nargs == 0 {
        return Err(arg_error(vm, 1, "assert", "value expected"));
    }
    let v = vm.nat_arg(fs, nargs, 0);
    if v.truthy() {
        // assert returns all its arguments
        let vals: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
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
    // PUC 5.5 `luaG_errormsg` substitutes "<no error object>" for nil;
    // earlier dialects propagate the nil unchanged (5.4 errors.lua :49
    // asserts `doit("error()") == nil`).
    if msg.is_nil() {
        if vm.version() >= crate::version::LuaVersion::Lua55 {
            let s = Value::Str(vm.heap.intern(b"<no error object>"));
            return Err(LuaError(s));
        }
        return Err(LuaError(Value::Nil));
    }
    let level = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => 1,
        v => vm.int_from(v, "use as a level")?,
    };
    if level <= 0 {
        // level 0: no position information added
        return Err(LuaError(msg));
    }
    // PUC `luaB_error` calls `luaL_where(L, level)` — prepend the position of
    // the Lua frame `level` steps up. If the level is out of range or the
    // target frame has no line info, fall through with no prefix.
    match msg {
        Value::Str(s) => {
            let prefix = vm.position_prefix_at_level(level);
            let text = match prefix {
                Some(p) => {
                    let mut t = p.into_bytes();
                    t.extend_from_slice(s.as_bytes());
                    t
                }
                None => s.as_bytes().to_vec(),
            };
            Err(LuaError(Value::Str(vm.heap.intern(&text))))
        }
        v => Err(LuaError(v)),
    }
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

pub(crate) fn nat_pcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    // PUC ≤5.3 `luaB_print` looks the `tostring` global up at call time
    // and calls *that* for each argument — so reassigning `tostring`
    // changes what `print` does (calls.lua 5.3 :29 sets `_ENV.tostring = nil`
    // and expects `print` to fail with "attempt to call a nil value").
    // 5.4 (`p2tostring`) inlined a private fast path that doesn't consult
    // the global, so luna's existing native shortcut matches it.
    let global_tostring = if vm.version() <= crate::version::LuaVersion::Lua53 {
        let key = Value::Str(vm.heap.intern(b"tostring"));
        let v = vm.globals().get(key);
        Some(v)
    } else {
        None
    };
    let mut out = Vec::new();
    for i in 0..nargs {
        if i > 0 {
            out.push(b'\t');
        }
        let v = vm.nat_arg(fs, nargs, i);
        let bytes = if let Some(ts) = global_tostring {
            let r = vm.call_value(ts, &[v])?;
            let s = r.into_iter().next().unwrap_or(Value::Nil);
            match s {
                Value::Str(s) => s.as_bytes().to_vec(),
                _ => {
                    return Err(raise_str(
                        vm,
                        "'tostring' must return a string to 'print'",
                    ));
                }
            }
        } else {
            vm.tostring_value(v)?
        };
        out.extend(bytes);
    }
    out.push(b'\n');
    let _ = std::io::stdout().write_all(&out);
    Ok(0)
}

fn nat_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(vm, 1, "tostring", "value expected"));
    }
    let v = vm.nat_arg(fs, nargs, 0);
    // PUC ≤5.2: `tostring(x)` returns whatever `__tostring` returns — even
    // non-string values like nil. PUC 5.3+ raises "must return a string".
    if vm.version() <= crate::version::LuaVersion::Lua52 {
        use crate::vm::exec::Mm;
        let mm = vm.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            let r = vm.call_value(mm, &[v])?;
            let out = r.into_iter().next().unwrap_or(Value::Nil);
            return Ok(vm.nat_return(fs, &[out]));
        }
    }
    // Fast-path Int: avoid the `i.to_string()` String allocation that
    // `tostring_value` would do — stack-buffer it then intern in place.
    if let Value::Int(i) = v {
        let mut buf = [0u8; 20];
        let bytes = crate::numeric::write_i64_dec(i, &mut buf);
        let s = Value::Str(vm.heap.intern(bytes));
        return Ok(vm.nat_return(fs, &[s]));
    }
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
    match unsafe { t.as_mut() }.set(&mut vm.heap, k, v) {
        Ok(()) => {
            vm.barrier_back_table(t);
            Ok(vm.nat_return(fs, &[tv]))
        }
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
    // setmetatable links a long-lived table to a long-lived mt; barrier_back
    // so the new mt gets traced even if t was already black.
    vm.barrier_back_table(t);
    // register for finalization if the new metatable carries `__gc` (PUC marks
    // the object finalizable at setmetatable time)
    vm.check_finalizer(t);
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

pub(crate) fn nat_pairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    // PUC luaB_pairs: a `__pairs` metamethod overrides the default iterator,
    // returning up to four values (iterator, state, control, to-be-closed).
    let mm = match vm.metatable_of(t) {
        Some(mt) => {
            let key = Value::Str(vm.heap.intern(b"__pairs"));
            mt.get(key)
        }
        None => Value::Nil,
    };
    if !mm.is_nil() {
        let res = vm.call_value(mm, &[t])?;
        let mut out = [Value::Nil; 4];
        for (slot, v) in out.iter_mut().zip(res) {
            *slot = v;
        }
        return Ok(vm.nat_return(fs, &out));
    }
    check_table(vm, t, "pairs")?;
    let it = vm.nat_upval(fs, 0);
    Ok(vm.nat_return(fs, &[it, t, Value::Nil]))
}

/// PUC `ipairsaux` — the iterator behind `ipairs`. Exposed
/// `pub(crate)` so the trace JIT (`Vm::jit_op_tforcall`) can
/// fn-pointer-compare against it for the v3 fast path (skip
/// `begin_call` + `nat_arg` and call `Table::get_int` directly).
pub(crate) fn ipairs_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    let i = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(i) => i,
        v => vm.int_from(v, "use as an index")?,
    };
    // ipairs over i64::MAX returns nil at the boundary (PUC's wrapping is
    // fine in release but debug builds panic without the explicit guard).
    let next_i = i.wrapping_add(1);
    // PUC ipairsaux uses lua_geti, honouring __index.
    let v = vm.index_value(tv, Value::Int(next_i))?;
    if v.is_nil() {
        Ok(vm.nat_return(fs, &[Value::Nil]))
    } else {
        Ok(vm.nat_return(fs, &[Value::Int(next_i), v]))
    }
}

fn nat_ipairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    // PUC 5.2 honoured `__ipairs(t)` — calling it returned the iterator
    // triplet. 5.3 dropped the metamethod entirely. nextvar.lua 5.2 :459
    // installs a proxy table whose `__ipairs` paginates an integer count.
    // luna doesn't track `Mm::Ipairs`, so look it up by name directly
    // (the path is cold — only hit when `t`'s metatable carries the key).
    if vm.version() == crate::version::LuaVersion::Lua52
        && let Value::Table(tt) = t
        && let Some(mt) = tt.metatable()
    {
        let key = Value::Str(vm.heap.intern(b"__ipairs"));
        let mm = mt.get(key);
        if !mm.is_nil() {
            let rs = vm.call_value(mm, &[t])?;
            let mut out: Vec<Value> = rs.into_iter().take(3).collect();
            while out.len() < 3 {
                out.push(Value::Nil);
            }
            return Ok(vm.nat_return(fs, &out));
        }
    }
    check_table(vm, t, "ipairs")?;
    let it = vm.nat_upval(fs, 0);
    Ok(vm.nat_return(fs, &[it, t, Value::Int(0)]))
}

// ---- shared helpers for the library modules ----

/// PUC luaL_argerror shape: bad argument #n to 'who' (extra). When the running
/// function was invoked as a method (`obj:m()`), the self argument isn't
/// counted: a bad `#1` becomes "calling 'm' on bad self". When the running
/// native was itself called from another native (PUC ar.name == NULL at level
/// 0, because the level-0 caller is C), the name is qualified via
/// `pushglobalfuncname` (e.g. `'sort'` → `'table.sort'`).
pub(crate) fn arg_error(vm: &mut Vm, n: u32, who: &str, extra: &str) -> LuaError {
    if let Some(("method", name)) = vm.running_call_name() {
        let n = n - 1; // self is not counted
        if n == 0 {
            return raise_str(vm, &format!("calling '{name}' on bad self ({extra})"));
        }
        return raise_str(vm, &format!("bad argument #{n} to '{name}' ({extra})"));
    }
    // The running native is the topmost on `running_natives`; a nested call
    // (depth ≥ 2) means the level-0 caller is another native, not Lua —
    // PUC walks package.loaded to qualify the running function's name.
    let name = if vm.running_natives.len() >= 2 {
        let target = vm.running_natives.last().expect("nested native").f;
        vm.pushglobalfuncname(target).unwrap_or_else(|| who.to_string())
    } else {
        who.to_string()
    };
    raise_str(vm, &format!("bad argument #{n} to '{name}' ({extra})"))
}

pub(crate) fn nat_tonumber(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(vm, 1, "tonumber", "value expected"));
    }
    let v = vm.nat_arg(fs, nargs, 0);
    if nargs < 2 || vm.nat_arg(fs, nargs, 1).is_nil() {
        // ≤5.2 has no integer subtype, so `tonumber("0xff…")` must return a
        // Float (PUC's `lua_strx2number` uses a double accumulator). 5.3+
        // keeps the int parse path so big hex wraps modulo 2^64 (matching
        // luna's literal lexer).
        let int_ok = vm.version() >= crate::version::LuaVersion::Lua53;
        let out = match v {
            Value::Int(_) | Value::Float(_) => v,
            Value::Str(s) => match crate::numeric::str2num(s.as_bytes(), int_ok, true) {
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
    // the chunk is either a source string or a reader function called until it
    // returns nil / no value / the empty string (PUC `load`).
    let (src_bytes, default_name): (Vec<u8>, Vec<u8>) = match chunk {
        Value::Str(s) => (s.as_bytes().to_vec(), s.as_bytes().to_vec()),
        Value::Closure(_) | Value::Native(_) => {
            // PUC's parser reads from the reader incrementally — it lexes
            // one token at a time with a one-char lookahead, so even an
            // immediately-syntactically-invalid chunk like `"*a = 123"`
            // pulls exactly 2 reader calls before the parser bails. luna's
            // parser is whole-buffer, so we approximate by trying a parse
            // after the first 2 bytes arrive: a *definitive* syntax error
            // (not an "expected <eof>" / "near '<eof>'" wall that just
            // signals "needs more input") returns immediately, with the
            // reader having been called exactly twice. 5.1 calls.lua :250
            // pins `i == 2`.
            let mut buf = Vec::new();
            let mut try_early = true;
            loop {
                // the reader runs in a protected context (PUC protectedparser):
                // an error it raises becomes a soft load failure
                let r = match vm.call_value(chunk, &[]) {
                    Ok(r) => r,
                    Err(e) => return Ok(vm.nat_return(fs, &[Value::Nil, e.0])),
                };
                match r.first() {
                    None | Some(Value::Nil) => break,
                    Some(Value::Str(s)) if s.as_bytes().is_empty() => break,
                    Some(Value::Str(s)) => buf.extend_from_slice(s.as_bytes()),
                    Some(_) => {
                        // a non-string from the reader is a soft load failure
                        let m = Value::Str(vm.heap.intern(b"reader function must return a string"));
                        return Ok(vm.nat_return(fs, &[Value::Nil, m]));
                    }
                }
                if try_early && buf.len() >= 2 && !crate::vm::dump::is_binary_chunk(&buf) {
                    try_early = false;
                    let ver = vm.version();
                    if let Err(e) = crate::frontend::parse(&buf, ver) {
                        let msg_str = String::from_utf8_lossy(&e.msg);
                        let eof_related =
                            msg_str.contains("<eof>") || msg_str.contains("near eof");
                        if !eof_related {
                            // definitive error — leave the source as is; the
                            // post-loop parse at the same call site re-runs
                            // it and produces the user-facing failure.
                            break;
                        }
                    }
                }
            }
            (buf, b"=(load)".to_vec())
        }
        _ => {
            return Err(arg_error(
                vm,
                1,
                "load",
                &format!("string expected, got {}", chunk.type_name()),
            ));
        }
    };
    let name: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => default_name,
    };
    // mode (arg 2): 't' allows text, 'b' allows binary; default "bt" allows both
    let mode = match vm.nat_arg(fs, nargs, 2) {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => b"bt".to_vec(),
    };
    // Lua-level load only accepts 't'/'b'; other chars (e.g. 'B' = a C-only
    // fixed-buffer chunk) are rejected as an argument error (PUC 5.5).
    if mode.iter().any(|c| !matches!(c, b'b' | b't')) {
        return Err(raise_str(
            vm,
            &format!("invalid mode '{}'", String::from_utf8_lossy(&mode)),
        ));
    }
    let binary = crate::vm::dump::is_binary_chunk(&src_bytes);
    if binary && !mode.contains(&b'b') || !binary && !mode.contains(&b't') {
        let kind = if binary { "binary" } else { "text" };
        let msg = format!(
            "attempt to load a {kind} chunk (mode is '{}')",
            String::from_utf8_lossy(&mode)
        );
        let m = Value::Str(vm.heap.intern(msg.as_bytes()));
        return Ok(vm.nat_return(fs, &[Value::Nil, m]));
    }
    match vm.load(&src_bytes, &name) {
        Ok(cl) => {
            if nargs >= 4 {
                let env = vm.nat_arg(fs, nargs, 3);
                let uv = vm.heap.new_upvalue(crate::runtime::UpvalState::Closed(env));
                unsafe { cl.as_mut() }.upvals_mut()[0] = uv;
            }
            Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
        }
        Err(e) => {
            // PUC formats the syntax error's source prefix via `luaO_chunkid`
            // (LUA_IDSIZE=60), not as a bare `[string "<name>"]`. This handles
            // the `@file` / `=name` sigils and head/tail-truncation rules.
            // `e.msg` carries raw bytes (PUC's near-token may be a non-UTF-8
            // byte from the source) — splice it in as-is so 5.1 errors.lua
            // can pattern-match `near '\xff'` etc.
            let display = crate::vm::lib_debug::chunk_id(&name);
            let mut msg_bytes = display;
            msg_bytes.push(b':');
            msg_bytes.extend_from_slice(e.line.to_string().as_bytes());
            msg_bytes.extend_from_slice(b": ");
            msg_bytes.extend_from_slice(&e.msg);
            let m = Value::Str(vm.heap.intern(&msg_bytes));
            Ok(vm.nat_return(fs, &[Value::Nil, m]))
        }
    }
}

/// Objects swept per unit of `collectgarbage("step", n)` step size. The PUC
/// step argument is in KB; we pace the incremental sweep by object count, so
/// this scales `n` into a per-step object budget.
const GC_STEP_OBJS: usize = 32;

/// PUC 5.1 `newproxy(...)`: create an empty userdata whose only purpose is to
/// carry a metatable (for `__index` / `__newindex` / `__gc` hooks).
///   - `newproxy()` / `newproxy(false)` ↦ no metatable
///   - `newproxy(true)` ↦ a fresh empty metatable
///   - `newproxy(other_userdata)` ↦ share the metatable of `other_userdata`
///
/// 5.2 retired the global; events.lua / gc.lua's metamethod sections use it
/// to attach proxies to test `__index`-table dispatch.
fn nat_newproxy(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::userdata::UserdataPayload;
    let arg = vm.nat_arg(fs, nargs, 0);
    let mt = match arg {
        Value::Bool(true) => Some(vm.heap.new_table()),
        Value::Userdata(other) => other.metatable(),
        // PUC's `newproxy` only accepts `nil` / `false` / `true` / a userdata.
        // Anything else raises "boolean or proxy expected".
        Value::Bool(false) | Value::Nil => None,
        v => {
            return Err(arg_error(
                vm,
                1,
                "newproxy",
                &format!("boolean or proxy expected, got {}", v.type_name()),
            ));
        }
    };
    let u = vm.heap.new_userdata(UserdataPayload::Empty, false);
    if let Some(mt) = mt {
        unsafe { u.as_mut() }.set_metatable(Some(mt));
        // PUC 5.1 registered *every* userdata with a metatable for
        // finalization (`luaC_checkfinalizer` deferred the `__gc` check to
        // GC time, so adding `__gc` to a shared metatable *after* the
        // proxy was made still works). 5.2+ moved the check to
        // setmetatable time; that's gated by version when needed.
        vm.heap.register_finalizable_userdata(u);
    }
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// PUC 5.1 `gcinfo()` — memory in use, in KB (an integer in PUC, which had
/// no integer subtype yet; luna mirrors the rounding). Replaced in 5.2+ by
/// `collectgarbage("count")`. gc.lua 5.1 :88 uses it as a loop guard.
fn nat_gcinfo(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let kb = (vm.heap.bytes() as f64 / 1024.0).floor() as i64;
    Ok(vm.nat_return(fs, &[Value::Int(kb)]))
}

/// PUC 5.1 `setfenv(f|level, env)`: replace the env of the Lua function `f`
/// (or of the Lua function at stack `level`). Writes through cell 0 of the
/// closure (which the 5.1 `Op::Closure` path keeps per-closure, so the
/// rewrite only affects this specific function — not its siblings).
fn nat_setfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let arg0 = vm.nat_arg(fs, nargs, 0);
    let env = vm.nat_arg(fs, nargs, 1);
    let env_table = match env {
        Value::Table(_) => env,
        v => {
            return Err(arg_error(
                vm,
                2,
                "setfenv",
                &format!("table expected, got {}", v.type_name()),
            ));
        }
    };
    let cl = match arg0 {
        Value::Closure(c) => c,
        Value::Native(_) => {
            // C functions have no upvalue 0 to rewrite (PUC raises this).
            return Err(raise_str(
                vm,
                "'setfenv' cannot change environment of given object",
            ));
        }
        Value::Int(i) => {
            if i == 0 {
                // PUC 5.1 `setfenv(0, env)` rewrites the thread's global
                // table (`L->l_gt`). luna repoints `Vm.globals` so future
                // `vm.load` calls snapshot the new table into the loaded
                // chunk's `_ENV` cell; already-loaded closures keep their
                // own per-closure `_ENV` cell. locals.lua's `foo("")`
                // probe relies on the loaded-chunk path picking up the new
                // table.
                if let Value::Table(t) = env_table {
                    vm.set_globals(t);
                }
                return Ok(vm.nat_return(fs, &[Value::Nil]));
            }
            // setfenv(1) targets the caller of setfenv. `dbg_frame(1)` already
            // skips continuation/C frames, so the running Lua frame at depth 1
            // *is* that caller — no +1 needed.
            let level = i;
            match vm.lua_closure_at_level(level) {
                Some(c) => c,
                None => return Err(arg_error(vm, 1, "setfenv", "invalid level")),
            }
        }
        Value::Float(f) => {
            let i = f as i64;
            if (i as f64) != f {
                return Err(arg_error(
                    vm,
                    1,
                    "setfenv",
                    "number has no integer representation",
                ));
            }
            if i == 0 {
                if let Value::Table(t) = env_table {
                    vm.set_globals(t);
                }
                return Ok(vm.nat_return(fs, &[Value::Nil]));
            }
            let level = i;
            match vm.lua_closure_at_level(level) {
                Some(c) => c,
                None => return Err(arg_error(vm, 1, "setfenv", "invalid level")),
            }
        }
        v => {
            return Err(arg_error(
                vm,
                1,
                "setfenv",
                &format!("number expected, got {}", v.type_name()),
            ));
        }
    };
    // Overwrite the closure's _ENV cell value. The 5.1 Op::Closure path made
    // this cell closed and unique to `cl`, so siblings stay untouched. The
    // `_ENV` upvalue is *not* guaranteed to sit at slot 0 — closures capture
    // upvalues in first-access order, so a body that touches a local
    // upvalue (e.g. `local saved = print; saved("hi"); module(...)`) puts
    // the local ahead of `_ENV`. Locate `_ENV` by name in the proto.
    let env_idx = cl
        .proto
        .upvals
        .iter()
        .position(|d| &*d.name == "_ENV")
        .ok_or_else(|| raise_str(vm, "'setfenv' target has no '_ENV' upvalue"))?;
    let uv = cl.upvals()[env_idx];
    unsafe { uv.as_mut() }.set_closed(env_table);
    vm.barrier_forward_upvalue(uv, env_table);
    Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
}

/// PUC 5.1 `getfenv(f|level)`: return the env of the Lua function `f` (or of
/// the Lua function at stack `level`). For a C function or `level == 0`, PUC
/// returns the thread's globals — luna's globals table.
fn nat_getfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC `getfunc(L, opt=1)`: `getfenv`'s argument is optional — a missing
    // arg AND an explicit `nil` both fall back to level 1 (the caller).
    let arg0 = if nargs == 0 {
        Value::Int(1)
    } else {
        match vm.nat_arg(fs, nargs, 0) {
            Value::Nil => Value::Int(1),
            v => v,
        }
    };
    // PUC `getfunc` raises "no function environment for tail call" when the
    // selected stack level is a CIST_TAIL placeholder — 5.1 db.lua :336.
    let level_arg: Option<i64> = match arg0 {
        Value::Closure(_) | Value::Native(_) => None,
        Value::Int(i) => Some(i),
        Value::Float(f) => {
            let i = f as i64;
            if (i as f64) != f {
                return Err(arg_error(vm, 1, "getfenv", "number has no integer representation"));
            }
            Some(i)
        }
        v => {
            return Err(arg_error(
                vm,
                1,
                "getfenv",
                &format!("number expected, got {}", v.type_name()),
            ));
        }
    };
    let cl = match arg0 {
        Value::Closure(c) => Some(c),
        _ => match level_arg {
            None | Some(0) => None,
            Some(level) => {
                use crate::vm::exec::DbgKind;
                match vm.dbg_frame(level) {
                    Some(DbgKind::Tail(_)) => {
                        return Err(raise_str(vm, "no function environment for tail call"));
                    }
                    Some(DbgKind::Lua(_)) => vm.lua_closure_at_level(level),
                    Some(DbgKind::C(_)) | None => None,
                }
            }
        },
    };
    use crate::runtime::UpvalState;
    let env = match cl {
        Some(c) => {
            let env_idx = c
                .proto
                .upvals
                .iter()
                .position(|d| &*d.name == "_ENV");
            match env_idx {
                Some(i) => match c.upvals()[i].state() {
                    UpvalState::Closed(v) => v,
                    UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
                },
                None => Value::Table(vm.globals()),
            }
        }
        None => Value::Table(vm.globals()),
    };
    Ok(vm.nat_return(fs, &[env]))
}

/// PUC 5.4+ `warn(msg1, ..., msgN)` — every argument must be a string. The
/// first N-1 pieces are emitted with `to_cont = true`, the last with
/// `to_cont = false`, so the default warnf concatenates the parts and flushes
/// the line at the tail call (mirrors `lbaselib.c::luaB_warn`).
pub(crate) fn nat_warn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(
            vm,
            1,
            "warn",
            "string expected, got no value",
        ));
    }
    let mut parts: Vec<Vec<u8>> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        match v {
            Value::Str(s) => parts.push(s.as_bytes().to_vec()),
            v => {
                return Err(arg_error(
                    vm,
                    i + 1u32,
                    "warn",
                    &format!("string expected, got {}", v.type_name()),
                ));
            }
        }
    }
    let n = parts.len();
    for (i, p) in parts.iter().enumerate() {
        vm.emit_warn(p, i + 1 < n);
    }
    Ok(vm.nat_return(fs, &[]))
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
    // the collector is not reentrant: called from within a `__gc` finalizer,
    // collectgarbage reports fail (PUC lua_gc returns -1 → luaL_pushfail).
    if vm.gc_is_finalizing() {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    let out = match opt.as_slice() {
        b"collect" => {
            // PUC 5.1–5.3 propagated the first `__gc` error to the
            // `collectgarbage` caller; 5.4 introduced the `warn` plumbing
            // and switched to "warn then continue". gc.lua 5.1 :255, 5.2
            // :346, and 5.3 :360 all baseline on the older raise behaviour.
            if vm.version() <= crate::version::LuaVersion::Lua53 {
                vm.collect_garbage_propagating()?;
            } else {
                vm.collect_garbage();
            }
            Value::Int(0)
        }
        b"count" => {
            // PUC 5.2/5.3 `LUA_GCCOUNT` reported as two results: kilobytes
            // (float = total/1024) and the residual bytes (`LUA_GCCOUNTB`,
            // 0..1024). 5.4 collapsed this to the single kilobytes float —
            // gc.lua 5.2 :139 asserts `k*1024 == floor(k)*1024 + b` exactly.
            let bytes = vm.heap.bytes();
            let kb = bytes as f64 / 1024.0;
            if vm.version() <= crate::version::LuaVersion::Lua53 {
                let b = (bytes % 1024) as i64;
                return Ok(vm.nat_return(fs, &[Value::Float(kb), Value::Int(b)]));
            }
            Value::Float(kb)
        }
        // "step": advance the collector. In generational mode a step is a minor
        // collection — a full atomic pass, so weak values created since the last
        // step are cleared at once. In incremental mode it sweeps a budgeted
        // chunk (proportional to the explicit step size `n`, or to the stepsize
        // param when none is given) and returns true once a full cycle finishes;
        // a larger budget finishes a cycle in fewer steps (stepsize 0 = a single
        // unbounded step completing the whole cycle, PUC "stop-the-world").
        b"step" => {
            if vm.gc_mode_is_generational() {
                vm.collect_garbage();
                Value::Bool(false)
            } else {
                let budget = if nargs >= 2 {
                    let v = vm.nat_arg(fs, nargs, 1);
                    let n = vm.int_from(v, "use as a step size")?.max(0) as usize;
                    n.saturating_mul(GC_STEP_OBJS).max(GC_STEP_OBJS)
                } else {
                    let ss = vm.gc_stepsize();
                    if ss <= 0 {
                        usize::MAX
                    } else {
                        (ss as usize).saturating_mul(GC_STEP_OBJS)
                    }
                };
                Value::Bool(vm.gc_step(budget))
            }
        }
        // legacy on/off switches (PUC keeps them in 5.5): suspend/resume auto-GC
        b"stop" => {
            vm.heap.gc_set_stopped(true);
            Value::Int(0)
        }
        b"restart" => {
            vm.heap.gc_set_stopped(false);
            Value::Int(0)
        }
        // mode switches report the previous mode (PUC). The collector is still
        // stop-the-world mark-sweep; the mode is tracked for API fidelity.
        b"incremental" => {
            let prev = vm.gc_switch_mode("incremental");
            Value::Str(vm.heap.intern(prev.as_bytes()))
        }
        b"generational" => {
            let prev = vm.gc_switch_mode("generational");
            Value::Str(vm.heap.intern(prev.as_bytes()))
        }
        b"isrunning" => Value::Bool(!vm.heap.gc_is_stopped()),
        // PUC 5.1-5.4 pacing-parameter shortcuts (5.5 routes them through
        // `collectgarbage("param", …)`). Each takes a new value and returns
        // the previous one as an integer; luna keeps the round-trip but does
        // not retune the collector, mirroring how the `param` arm already
        // works. gc.lua 5.4 :31 cycles through `setpause`/`setstepmul`.
        b"setpause" => {
            let set = if nargs >= 2 {
                let v = vm.nat_arg(fs, nargs, 1);
                Some(vm.int_from(v, "use as a parameter")?)
            } else {
                None
            };
            Value::Int(vm.gc_param(b"pause", set).unwrap_or(0))
        }
        b"setstepmul" => {
            let set = if nargs >= 2 {
                let v = vm.nat_arg(fs, nargs, 1);
                Some(vm.int_from(v, "use as a parameter")?)
            } else {
                None
            };
            Value::Int(vm.gc_param(b"stepmul", set).unwrap_or(0))
        }
        b"setmajorinc" => {
            let set = if nargs >= 2 {
                let v = vm.nat_arg(fs, nargs, 1);
                Some(vm.int_from(v, "use as a parameter")?)
            } else {
                None
            };
            Value::Int(vm.gc_param(b"majormul", set).unwrap_or(0))
        }
        b"setstepsize" => {
            let set = if nargs >= 2 {
                let v = vm.nat_arg(fs, nargs, 1);
                Some(vm.int_from(v, "use as a parameter")?)
            } else {
                None
            };
            Value::Int(vm.gc_param(b"stepsize", set).unwrap_or(0))
        }
        // "param" reads, or sets and returns the previous value of, a pacing
        // parameter (PUC 5.5 collectgarbage("param", name [,value])). The
        // collector is stop-the-world, so values only round-trip for fidelity.
        b"param" => {
            let name = match vm.nat_arg(fs, nargs, 1) {
                Value::Str(s) => s.as_bytes().to_vec(),
                v => {
                    return Err(arg_error(
                        vm,
                        2,
                        "collectgarbage",
                        &format!("string expected, got {}", v.type_name()),
                    ));
                }
            };
            let set = if nargs >= 3 {
                let v = vm.nat_arg(fs, nargs, 2);
                Some(vm.int_from(v, "use as a parameter")?)
            } else {
                None
            };
            match vm.gc_param(&name, set) {
                Some(prev) => Value::Int(prev),
                None => {
                    let n = String::from_utf8_lossy(&name).into_owned();
                    return Err(arg_error(
                        vm,
                        2,
                        "collectgarbage",
                        &format!("invalid parameter '{n}'"),
                    ));
                }
            }
        }
        // PUC luaL_checkoption: an unrecognized option is an argument error.
        opt => {
            let o = String::from_utf8_lossy(opt).into_owned();
            return Err(arg_error(
                vm,
                1,
                "collectgarbage",
                &format!("invalid option '{o}'"),
            ));
        }
    };
    Ok(vm.nat_return(fs, &[out]))
}

pub(crate) fn nat_xpcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs < 2 {
        return Err(raise_str(
            vm,
            "bad argument #2 to 'xpcall' (value expected)",
        ));
    }
    let f = vm.nat_arg(fs, nargs, 0);
    let h = vm.nat_arg(fs, nargs, 1);
    let args: Vec<Value> = (2..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    match vm.call_value(f, &args) {
        Ok(results) => {
            let mut out = Vec::with_capacity(results.len() + 1);
            out.push(Value::Bool(true));
            out.extend(results);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => match vm.call_value(h, &[e.0]) {
            Ok(hr) => {
                let v = hr.first().copied().unwrap_or(Value::Nil);
                Ok(vm.nat_return(fs, &[Value::Bool(false), v]))
            }
            Err(_) => {
                // error while handling the error (PUC LUA_ERRERR)
                let m = Value::Str(vm.heap.intern(b"error in error handling"));
                Ok(vm.nat_return(fs, &[Value::Bool(false), m]))
            }
        },
    }
}
