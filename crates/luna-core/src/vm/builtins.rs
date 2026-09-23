//! Minimal base library — what the P03 gate corpus needs. The full base
//! library (P04) replaces/extends this.

use std::io::Write;

use crate::runtime::{Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_base(vm: &mut Vm) {
    let f = vm.native(nat_assert);
    vm.set_global("assert", f).expect("stdlib registration");
    let f = vm.native(nat_error);
    vm.set_global("error", f).expect("stdlib registration");
    let f = vm.native(nat_pcall);
    vm.set_global("pcall", f).expect("stdlib registration");
    let f = vm.native(nat_xpcall);
    vm.set_global("xpcall", f).expect("stdlib registration");
    let f = vm.native(nat_type);
    vm.set_global("type", f).expect("stdlib registration");
    let f = vm.native(nat_print);
    vm.set_global("print", f).expect("stdlib registration");
    let f = vm.native(nat_tostring);
    vm.set_global("tostring", f).expect("stdlib registration");
    let f = vm.native(nat_rawget);
    vm.set_global("rawget", f).expect("stdlib registration");
    let f = vm.native(nat_rawset);
    vm.set_global("rawset", f).expect("stdlib registration");
    let f = vm.native(nat_rawequal);
    vm.set_global("rawequal", f).expect("stdlib registration");
    // `rawlen` arrived in 5.2.
    if vm.version() >= crate::version::LuaVersion::Lua52 {
        let f = vm.native(nat_rawlen);
        vm.set_global("rawlen", f).expect("stdlib registration");
    }
    let f = vm.native(nat_setmetatable);
    vm.set_global("setmetatable", f)
        .expect("stdlib registration");
    let f = vm.native(nat_getmetatable);
    vm.set_global("getmetatable", f)
        .expect("stdlib registration");
    let f = vm.native(nat_select);
    vm.set_global("select", f).expect("stdlib registration");
    let next_obj = vm.native(nat_next);
    vm.set_global("next", next_obj)
        .expect("stdlib registration");
    // 5.2+ pairs returns the global next itself (a light C function, equal
    // to every other push of `luaB_next`); 5.1 has no light C functions and
    // gives pairs its own closure over `luaB_next`, so `pairs{} ~= next`.
    let pairs_next = if vm.version() == LuaVersion::Lua51 {
        vm.native(nat_next)
    } else {
        next_obj
    };
    let pairs_obj = vm.native_with(nat_pairs, Box::new([pairs_next]));
    vm.set_global("pairs", pairs_obj)
        .expect("stdlib registration");
    let ipairs_it = vm.native(ipairs_iter);
    let ipairs_obj = vm.native_with(nat_ipairs, Box::new([ipairs_it]));
    vm.set_global("ipairs", ipairs_obj)
        .expect("stdlib registration");
    let f = vm.native(nat_tonumber);
    vm.set_global("tonumber", f).expect("stdlib registration");
    let load_obj = vm.native(nat_load);
    vm.set_global("load", load_obj)
        .expect("stdlib registration");
    let f = vm.native(nat_collectgarbage);
    vm.set_global("collectgarbage", f)
        .expect("stdlib registration");
    // PUC 5.4 introduced the warning system. `warn(msg1, …, msgN)` emits
    // pieces of one message via the default warnf (`lauxlib.c::warnfon/off`),
    // which recognises `@on` / `@off` control messages and starts disabled.
    if vm.version() >= crate::version::LuaVersion::Lua54 {
        let f = vm.native(nat_warn);
        vm.set_global("warn", f).expect("stdlib registration");
    }
    // PUC 5.2's official build ships -DLUA_COMPAT_ALL, so `loadstring`
    // survives as a `load` alias there too — the diff ground truth is
    // the default build (v2.14 dialect fixture 5.2/521).
    if vm.version() == crate::version::LuaVersion::Lua52 {
        vm.set_global("loadstring", load_obj)
            .expect("stdlib registration");
    }
    // PUC 5.1 globals retired in 5.2 (`unpack` → `table.unpack`) and 5.2
    // (`loadstring` → `load`). Provide aliases so the 5.1 test suite, which
    // is full of `unpack(...)` and `loadstring("...")` calls, still resolves.
    if vm.version() == crate::version::LuaVersion::Lua51 {
        vm.set_global("loadstring", load_obj)
            .expect("stdlib registration");
        let f = vm.native(crate::vm::lib_table::t_unpack);
        vm.set_global("unpack", f).expect("stdlib registration");
        // PUC 5.1 also exposed `gcinfo()` (memory in KB) and `newproxy()`
        // (debug-table proxy with `__gc`). gcinfo is a thin wrapper around
        // `collectgarbage("count")`; newproxy is left in the backlog —
        // its `__gc` finalizer integration is non-trivial.
        let f = vm.native(nat_gcinfo);
        vm.set_global("gcinfo", f).expect("stdlib registration");
        // PUC 5.1 `setfenv`/`getfenv` — every Lua function carries its own
        // env (5.1 `LClosure.env`); 5.2 retired them in favour of the `_ENV`
        // upvalue model. The Op::Closure path here clones cell 0 per
        // closure under 5.1, so writing through the per-closure cell only
        // affects that closure (events.lua / locals.lua / nextvar.lua).
        let f = vm.native(nat_setfenv);
        vm.set_global("setfenv", f).expect("stdlib registration");
        let f = vm.native(nat_getfenv);
        vm.set_global("getfenv", f).expect("stdlib registration");
        let f = vm.native(nat_newproxy);
        vm.set_global("newproxy", f).expect("stdlib registration");
    }
    let version = match vm.version() {
        crate::version::LuaVersion::Lua51 => "Lua 5.1",
        crate::version::LuaVersion::Lua52 => "Lua 5.2",
        crate::version::LuaVersion::Lua53 => "Lua 5.3",
        crate::version::LuaVersion::Lua54 => "Lua 5.4",
        // MacroLua reports the 5.4 base it inherits from (audit-locked).
        crate::version::LuaVersion::MacroLua => "Lua 5.4",
        crate::version::LuaVersion::Lua55 => "Lua 5.5",
    };
    let v = Value::Str(vm.heap.intern(version.as_bytes()));
    vm.set_global("_VERSION", v).expect("stdlib registration");
    let g = Value::Table(vm.globals());
    vm.set_global("_G", g).expect("stdlib registration");
}

/// `luaL_checktype(L, 1, LUA_TTABLE)` on a value already read from
/// argument 1, for the table library's 5.1-only functions.
pub(crate) fn check_table(vm: &mut Vm, v: Value) -> Result<crate::runtime::Gc<Table>, LuaError> {
    match v {
        Value::Table(t) => Ok(t),
        v => {
            let got = vm.obj_typename(v);
            Err(arg_error(vm, 1, &format!("table expected, got {got}")))
        }
    }
}

fn nat_assert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = a.get(vm, 0);
    if v.truthy() {
        // assert returns all its arguments
        let vals: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
        return Ok(vm.nat_return(fs, &vals));
    }
    match vm.version() {
        // 5.1 checks for a condition first; 5.2 does not, so a bare
        // `assert()` fails the assertion. Both format the message with
        // `luaL_optstring` (numbers convert, anything else is an argument
        // error) and raise it through `luaL_error`.
        LuaVersion::Lua51 | LuaVersion::Lua52 => {
            if vm.version() == LuaVersion::Lua51 {
                argcheck::check_any(vm, a, 0)?;
            }
            match argcheck::opt_string(vm, a, 1)? {
                Some(msg) => Err(raise(vm, Value::Str(msg))),
                None => Err(raise_str(vm, "assertion failed!")),
            }
        }
        // 5.3+ hands the message, of any type, to `error` at level 1; an
        // explicit nil message stays nil (`lua_settop(L, 1)` keeps it).
        _ => {
            argcheck::check_any(vm, a, 0)?;
            if nargs >= 2 {
                Err(raise(vm, a.get(vm, 1)))
            } else {
                Err(raise_str(vm, "assertion failed!"))
            }
        }
    }
}

fn nat_error(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // The level is read before anything else, so a bad level is reported
    // whatever the message is.
    let level = argcheck::opt_int(vm, a, 1, 1)?;
    let msg = a.get(vm, 0);
    // A nil error object stays nil HERE: PUC 5.5's luaG_errormsg
    // substitutes "<no error object>" only AFTER the message handler
    // ran (ldebug.c:849-852) — xpcall handlers and the standalone
    // msghandler see the raw nil ("(error object is a nil value)" at
    // top level, v2.14 fixture 5.5/334), while a plain pcall catch
    // yields the substituted string. luna's substitution lives at the
    // matching point in `unwind` (5.5-gated there).
    if level <= 0 {
        return Err(LuaError(msg));
    }
    // ≤5.2 tests `lua_isstring`, so a number message is positioned too and
    // comes out as a string; 5.3+ positions only real strings.
    let text = match msg {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(_) | Value::Float(_) if vm.version() <= LuaVersion::Lua52 => {
            argcheck::to_str_bytes(vm, msg).expect("a number converts to a string")
        }
        v => return Err(LuaError(v)),
    };
    // PUC `luaB_error` calls `luaL_where(L, level)` — prepend the position of
    // the Lua frame `level` steps up. If the level is out of range or the
    // target frame has no line info, fall through with no prefix.
    let mut out = vm
        .position_prefix_at_level(level as i64)
        .map(String::into_bytes)
        .unwrap_or_default();
    out.extend_from_slice(&text);
    Err(LuaError(Value::Str(vm.heap.intern(&out))))
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

/// `pcall` reached through `call_value`; a call from Lua goes through
/// `Vm::begin_pcall` instead, which makes the protected call yieldable.
pub(crate) fn nat_pcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_any(vm, a, 0)?;
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
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    let s = Value::Str(vm.heap.intern(v.type_name().as_bytes()));
    Ok(vm.nat_return(fs, &[s]))
}

fn nat_print(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC ≤5.3 `luaB_print` looks the `tostring` global up at call time
    // and calls *that* for each argument — so reassigning `tostring`
    // changes what `print` does (calls.lua 5.3 :29 sets `_ENV.tostring = nil`
    // and expects `print` to fail with "attempt to call a nil value").
    // 5.4 (`luaL_tolstring`) converts in place without consulting the global.
    let global_tostring = if vm.version() <= LuaVersion::Lua53 {
        let g = Value::Table(vm.globals());
        let key = Value::Str(vm.heap.intern(b"tostring"));
        Some(vm.index_value(g, key)?)
    } else {
        None
    };
    let mut out = Vec::new();
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        let piece = match global_tostring {
            Some(ts) => match vm.call_value(ts, &[v]) {
                // `lua_tostring` on the result: a number is accepted and
                // rendered, anything else is refused.
                Ok(r) => match r.first().and_then(|&s| argcheck::to_str_bytes(vm, s)) {
                    Some(b) => Ok(b),
                    None => Err(raise_str(vm, "'tostring' must return a string to 'print'")),
                },
                Err(e) => Err(e),
            },
            None => vm.tostring_value(v),
        };
        // PUC writes each argument as soon as it is converted, so the ones
        // before a failing conversion still reach stdout.
        let piece = match piece {
            Ok(b) => b,
            Err(e) => {
                write_stdout(&out);
                return Err(e);
            }
        };
        if i > 0 {
            out.push(b'\t');
        }
        // 5.1 writes each piece with `fputs`, which stops at an embedded NUL;
        // 5.2+ writes the full length.
        let piece = match piece.iter().position(|&c| c == 0) {
            Some(nul) if vm.version() == LuaVersion::Lua51 => &piece[..nul],
            _ => &piece[..],
        };
        out.extend_from_slice(piece);
    }
    out.push(b'\n');
    write_stdout(&out);
    Ok(0)
}

fn write_stdout(bytes: &[u8]) {
    // PUC's `lua_writestring` is an unchecked `fwrite`: a closed or full
    // stdout does not make `print` fail.
    let _ = std::io::stdout().write_all(bytes);
}

fn nat_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    // Fast-path Int: avoid the `i.to_string()` String allocation that
    // `tostring_value` would do — stack-buffer it then intern in place.
    // A number metatable (debug.setmetatable) could carry `__tostring`.
    if let Value::Int(i) = v
        && vm.metatable_of(v).is_none()
    {
        let mut buf = [0u8; 20];
        let bytes = crate::numeric::write_i64_dec(i, &mut buf);
        let s = Value::Str(vm.heap.intern(bytes));
        return Ok(vm.nat_return(fs, &[s]));
    }
    // PUC ≤5.2: `tostring(x)` returns whatever `__tostring` returns — even
    // non-string values like nil. 5.1 hands it back untouched; 5.2 goes
    // through `lua_tolstring`, which renders a number as a string. 5.3+
    // raises "must return a string" (in `tostring_value`).
    if vm.version() <= LuaVersion::Lua52 {
        use crate::vm::exec::Mm;
        let mm = vm.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            let r = vm.call_value(mm, &[v])?;
            let mut out = r.into_iter().next().unwrap_or(Value::Nil);
            if vm.version() == LuaVersion::Lua52
                && let Some(b) = match out {
                    Value::Int(_) | Value::Float(_) => argcheck::to_str_bytes(vm, out),
                    _ => None,
                }
            {
                out = Value::Str(vm.heap.intern(&b));
            }
            return Ok(vm.nat_return(fs, &[out]));
        }
    }
    let bytes = vm.tostring_value(v)?;
    let s = Value::Str(vm.heap.intern(&bytes));
    Ok(vm.nat_return(fs, &[s]))
}

fn nat_rawget(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let k = argcheck::check_any(vm, a, 1)?;
    let v = t.get(k);
    Ok(vm.nat_return(fs, &[v]))
}

fn nat_rawset(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let k = argcheck::check_any(vm, a, 1)?;
    let v = argcheck::check_any(vm, a, 2)?;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    match unsafe { t.as_mut() }.set(&mut vm.heap, k, v) {
        Ok(()) => {
            vm.barrier_back_table(t);
            Ok(vm.nat_return(fs, &[Value::Table(t)]))
        }
        // `luaG_runerror` inside `lua_rawset`: the running function is C, so
        // no position is added.
        Err(crate::runtime::TableError::NilIndex) => Err(vm.plain_err("table index is nil")),
        Err(crate::runtime::TableError::NanIndex) => Err(vm.plain_err("table index is NaN")),
        Err(_) => unreachable!(),
    }
}

fn nat_rawequal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_any(vm, a, 0)?;
    let y = argcheck::check_any(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Bool(x.raw_eq(y))]))
}

fn nat_rawlen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let n = match a.get(vm, 0) {
        Value::Table(t) => t.len(),
        Value::Str(s) => s.len() as i64,
        _ => return Err(argcheck::arg_expected(vm, a, 0, "table or string")),
    };
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

fn nat_setmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let mt = match a.get(vm, 1) {
        _ if a.is_none(1) => return Err(argcheck::arg_expected(vm, a, 1, "nil or table")),
        Value::Nil => None,
        Value::Table(m) => Some(m),
        _ => return Err(argcheck::arg_expected(vm, a, 1, "nil or table")),
    };
    if !vm.get_mm(Value::Table(t), Mm::Metatable).is_nil() {
        return Err(raise_str(vm, "cannot change a protected metatable"));
    }
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.set_metatable(mt);
    // setmetatable links a long-lived table to a long-lived mt; barrier_back
    // so the new mt gets traced even if t was already black.
    vm.barrier_back_table(t);
    // register for finalization if the new metatable carries `__gc` (PUC marks
    // the object finalizable at setmetatable time)
    vm.check_finalizer(t);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

fn nat_getmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    // __metatable protection: return that field instead
    let protected = vm.get_mm(v, Mm::Metatable);
    if !protected.is_nil() {
        return Ok(vm.nat_return(fs, &[protected]));
    }
    let mt = vm.metatable_of(v).map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[mt]))
}

fn nat_select(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let n = match a.get(vm, 0) {
        Value::Int(i) if vm.version() >= LuaVersion::Lua53 => i,
        // PUC tests only the first character: `select("#x", ...)` counts too.
        Value::Str(s) if s.as_bytes().first() == Some(&b'#') => {
            return Ok(vm.nat_return(fs, &[Value::Int(nargs as i64 - 1)]));
        }
        // ≤5.2 reads the index with `luaL_checkint`, a C int.
        _ if vm.version() <= LuaVersion::Lua52 => argcheck::check_int(vm, a, 0)? as i64,
        _ => argcheck::check_integer(vm, a, 0)?,
    };
    let top = nargs as i64;
    let i = if n < 0 {
        top + n
    } else if n > top {
        top
    } else {
        n
    };
    if i < 1 {
        return Err(arg_error(vm, 1, "index out of range"));
    }
    let vals: Vec<Value> = (i..top).map(|k| vm.nat_arg(fs, nargs, k as u32)).collect();
    Ok(vm.nat_return(fs, &vals))
}

fn nat_next(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let k = a.get(vm, 1);
    // 5.3+ tables keep integer keys only as integers, and `luaH_next` looks a
    // float key up without normalizing it: an integral float never matches.
    if let Value::Float(f) = k
        && vm.version() >= LuaVersion::Lua53
        && crate::runtime::value::f2i_exact(f).is_some()
    {
        return Err(vm.plain_err("invalid key to 'next'"));
    }
    match t.next(k) {
        Ok(Some((k, v))) => Ok(vm.nat_return(fs, &[k, v])),
        Ok(None) => Ok(vm.nat_return(fs, &[Value::Nil])),
        Err(_) => Err(vm.plain_err("invalid key to 'next'")),
    }
}

/// `pairs` without a `__pairs` metamethod (the dispatcher in exec.rs calls a
/// present `__pairs` yieldably through `Vm::begin_pairs`, which also covers
/// a `pairs` reached through `call_value` here).
pub(crate) fn nat_pairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    // 5.3+ take any value (the iterator fails later if it is no table).
    if ver >= LuaVersion::Lua53 {
        argcheck::check_any(vm, a, 0)?;
    }
    let t = a.get(vm, 0);
    if ver >= LuaVersion::Lua52 {
        let mm = vm.get_mm(t, Mm::Pairs);
        if !mm.is_nil() {
            let n = pairs_mm_results(vm);
            let res = vm.call_value(mm, &[t])?;
            let mut out = [Value::Nil; 4];
            for (slot, v) in out.iter_mut().zip(res) {
                *slot = v;
            }
            return Ok(vm.nat_return(fs, &out[..n]));
        }
    }
    if ver <= LuaVersion::Lua52 {
        argcheck::check_table(vm, a, 0)?;
    }
    let it = vm.nat_upval(fs, 0);
    // 5.5 adds a fourth value, the (nil) to-be-closed variable.
    if ver >= LuaVersion::Lua55 {
        Ok(vm.nat_return(fs, &[it, t, Value::Nil, Value::Nil]))
    } else {
        Ok(vm.nat_return(fs, &[it, t, Value::Nil]))
    }
}

/// How many results `pairs` takes from a `__pairs` metamethod: 5.5 keeps four
/// (the fourth is a to-be-closed value), 5.2–5.4 three.
pub(crate) fn pairs_mm_results(vm: &Vm) -> usize {
    if vm.version() >= LuaVersion::Lua55 {
        4
    } else {
        3
    }
}

/// PUC `ipairsaux` — the iterator behind `ipairs`. Exposed
/// `pub(crate)` so the trace JIT (`Vm::jit_op_tforcall`) can
/// fn-pointer-compare against it for the v3 fast path (skip
/// `begin_call` + `nat_arg` and call `Table::get_int` directly).
#[doc(hidden)]
pub fn ipairs_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let i = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(i) if vm.version() >= LuaVersion::Lua53 => i,
        _ if vm.version() <= LuaVersion::Lua52 => return ipairs_iter_raw(vm, fs, nargs),
        _ => argcheck::check_integer(vm, Args::new(fs, nargs), 1)?,
    };
    let tv = vm.nat_arg(fs, nargs, 0);
    // `luaL_intop(+, i, 1)`: wraps at the top of the integer range.
    let next_i = i.wrapping_add(1);
    // PUC 5.3+ ipairsaux uses lua_geti, honouring __index on any value.
    let v = vm.index_value(tv, Value::Int(next_i))?;
    if v.is_nil() {
        Ok(vm.nat_return(fs, &[Value::Nil]))
    } else {
        Ok(vm.nat_return(fs, &[Value::Int(next_i), v]))
    }
}

/// ≤5.2 `ipairsaux`: the control value is a C int read before the table is
/// checked, elements are read raw, and the end of the sequence is no values
/// on 5.1 but a single nil on 5.2.
#[cold]
fn ipairs_iter_raw(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let i = argcheck::check_int(vm, a, 1)?.wrapping_add(1);
    let t = argcheck::check_table(vm, a, 0)?;
    let v = t.get_int(i as i64);
    if !v.is_nil() {
        Ok(vm.nat_return(fs, &[Value::Int(i as i64), v]))
    } else if vm.version() == LuaVersion::Lua51 {
        Ok(vm.nat_return(fs, &[]))
    } else {
        Ok(vm.nat_return(fs, &[Value::Nil]))
    }
}

fn nat_ipairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    if ver >= LuaVersion::Lua53 {
        argcheck::check_any(vm, a, 0)?;
    }
    let t = a.get(vm, 0);
    // 5.2 honoured `__ipairs(t)`, and so does 5.3's default build
    // (LUA_COMPAT_5_2 → LUA_COMPAT_IPAIRS): the metamethod's first three
    // results replace the iterator triplet. 5.4 dropped it. nextvar.lua 5.2
    // :459 paginates a proxy through it.
    if (ver == LuaVersion::Lua52 || ver == LuaVersion::Lua53)
        && let Some(mt) = vm.metatable_of(t)
    {
        let key = Value::Str(vm.heap.intern(b"__ipairs"));
        let mm = mt.get(key);
        if !mm.is_nil() {
            let rs = vm.call_value(mm, &[t])?;
            let mut out = [Value::Nil; 3];
            for (slot, v) in out.iter_mut().zip(rs) {
                *slot = v;
            }
            return Ok(vm.nat_return(fs, &out));
        }
    }
    if ver <= LuaVersion::Lua52 {
        argcheck::check_table(vm, a, 0)?;
    }
    let it = vm.nat_upval(fs, 0);
    Ok(vm.nat_return(fs, &[it, t, Value::Int(0)]))
}

// ---- shared helpers for the library modules ----

/// PUC `luaL_argerror`: "bad argument #n to 'name' (extra)".
///
/// The name is the one the caller used (`lua_getinfo(L, "n")` at level 0),
/// so `local f = string.rep; f()` blames 'f'. A method call does not count
/// the self argument: a bad `#1` there becomes "calling 'm' on bad self".
/// When the caller gives no name — the native was called by another native
/// or by pcall, or through an unnamed expression — 5.2+ looks the function
/// up in `package.loaded` and 5.1 prints '?'.
pub(crate) fn arg_error(vm: &mut Vm, n: u32, extra: &str) -> LuaError {
    // A nested native, or a pcall/xpcall continuation directly below, means
    // the level-0 caller is C, which PUC never names.
    let called_from_c = vm.running_natives.len() >= 2 || vm.caller_is_protected_cont();
    let call_name = if called_from_c {
        None
    } else {
        vm.running_call_name()
    };
    let name = match call_name {
        Some(("method", name)) => {
            let n = n - 1; // self is not counted
            if n == 0 {
                return raise_str(vm, &format!("calling '{name}' on bad self ({extra})"));
            }
            return raise_str(vm, &format!("bad argument #{n} to '{name}' ({extra})"));
        }
        Some((_, name)) => name,
        None => unnamed_native_name(vm),
    };
    raise_str(vm, &format!("bad argument #{n} to '{name}' ({extra})"))
}

/// `luaL_argerror`'s fallback when `ar.name` is NULL: '?' on 5.1; otherwise
/// the running native's `package.loaded` name — kept whole on 5.2
/// (`'_G.tonumber'`), without the `_G.` prefix from 5.3 on — or '?'.
fn unnamed_native_name(vm: &mut Vm) -> String {
    if vm.version() == crate::version::LuaVersion::Lua51 {
        return "?".to_string();
    }
    let Some(target) = vm.running_natives.last().map(|nc| nc.f) else {
        return "?".to_string();
    };
    let name = if vm.version() == crate::version::LuaVersion::Lua52 {
        vm.loaded_funcname(target)
    } else {
        vm.pushglobalfuncname(target)
    };
    name.unwrap_or_else(|| "?".to_string())
}

pub(crate) fn nat_tonumber(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(arg_error(vm, 1, "value expected"));
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
        return Err(arg_error(vm, 2, "base out of range"));
    }
    let Value::Str(s) = v else {
        return Err(arg_error(
            vm,
            1,
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
            // Snapshot the loader budget once — embedders are not
            // expected to widen the cap mid-`load`.
            let input_budget = vm.loader_input_budget();
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
                    Some(Value::Str(s)) => {
                        let bytes = s.as_bytes();
                        // Gate the next chunk *before* we extend the
                        // buffer: PUC's `loadrep` feeder returns a 1 MiB
                        // string every iteration and runs forever; with
                        // the default 256 MiB cap we error out after the
                        // first quarter-gig instead of letting the host
                        // allocator crawl past 7 GB then SIGSEGV.
                        // Matches the PUC `not enough memory` failure
                        // shape that `heavy.lua::loadrep` asserts on.
                        if bytes.len() > input_budget.saturating_sub(buf.len()) {
                            let m = Value::Str(vm.heap.intern(b"not enough memory"));
                            return Ok(vm.nat_return(fs, &[Value::Nil, m]));
                        }
                        buf.extend_from_slice(bytes);
                    }
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
                        let eof_related = msg_str.contains("<eof>") || msg_str.contains("near eof");
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
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
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
                &format!("boolean or proxy expected, got {}", v.type_name()),
            ));
        }
    };
    let u = vm.heap.new_userdata(UserdataPayload::Empty, false);
    if let Some(mt) = mt {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
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
                None => return Err(arg_error(vm, 1, "invalid level")),
            }
        }
        Value::Float(f) => {
            let i = f as i64;
            if (i as f64) != f {
                return Err(arg_error(vm, 1, "number has no integer representation"));
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
                None => return Err(arg_error(vm, 1, "invalid level")),
            }
        }
        v => {
            return Err(arg_error(
                vm,
                1,
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
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
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
                return Err(arg_error(vm, 1, "number has no integer representation"));
            }
            Some(i)
        }
        v => {
            return Err(arg_error(
                vm,
                1,
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
            let env_idx = c.proto.upvals.iter().position(|d| &*d.name == "_ENV");
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

/// PUC 5.4+ `warn(msg1, ..., msgN)` — every argument must be a string (or a
/// number, which converts). The first N-1 pieces are emitted with
/// `to_cont = true`, the last with `to_cont = false`, so the default warnf
/// concatenates the parts and flushes the line at the tail call (mirrors
/// `lbaselib.c::luaB_warn`). All arguments are checked before any is
/// emitted, so a bad one leaves no half-composed warning.
pub(crate) fn nat_warn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let mut parts = Vec::with_capacity(nargs.max(1) as usize);
    for i in 0..nargs.max(1) {
        parts.push(argcheck::check_string(vm, a, i)?);
    }
    let n = parts.len();
    for (i, p) in parts.iter().enumerate() {
        vm.emit_warn(p.as_bytes(), i + 1 < n);
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
                    return Err(arg_error(vm, 2, &format!("invalid parameter '{n}'")));
                }
            }
        }
        // PUC luaL_checkoption: an unrecognized option is an argument error.
        opt => {
            let o = String::from_utf8_lossy(opt).into_owned();
            return Err(arg_error(vm, 1, &format!("invalid option '{o}'")));
        }
    };
    Ok(vm.nat_return(fs, &[out]))
}

/// `xpcall` reached through `call_value`; a call from Lua goes through
/// `Vm::begin_xpcall` instead, which makes the protected call yieldable.
pub(crate) fn nat_xpcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let h = xpcall_handler(vm, a)?;
    let f = vm.nat_arg(fs, nargs, 0);
    // 5.1 `xpcall(f, err)` calls `f` with no arguments.
    let first_arg = if vm.version() == LuaVersion::Lua51 {
        nargs
    } else {
        2
    };
    let args: Vec<Value> = (first_arg..nargs)
        .map(|i| vm.nat_arg(fs, nargs, i))
        .collect();
    match vm.call_value(f, &args) {
        Ok(results) => {
            let mut out = Vec::with_capacity(results.len() + 1);
            out.push(Value::Bool(true));
            out.extend(results);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => {
            let m = handle_error(vm, h, e.0);
            Ok(vm.nat_return(fs, &[Value::Bool(false), m]))
        }
    }
}

/// `xpcall`'s check of its message handler: ≤5.2 only needs a second
/// argument; 5.3+ requires a function.
pub(crate) fn xpcall_handler(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    if vm.version() >= LuaVersion::Lua53 {
        argcheck::check_function(vm, a, 1)
    } else {
        argcheck::check_any(vm, a, 1)
    }
}

/// Run `xpcall`'s message handler `h` on `err` for the `call_value` path.
/// On ≤5.2 a handler that is not a function cannot be called at all
/// (`luaG_errormsg` raises LUA_ERRERR), and an error inside the handler is
/// LUA_ERRERR too.
fn handle_error(vm: &mut Vm, h: Value, err: Value) -> Value {
    let callable =
        vm.version() >= LuaVersion::Lua53 || matches!(h, Value::Closure(_) | Value::Native(_));
    match callable.then(|| vm.call_value(h, &[err])) {
        Some(Ok(hr)) => hr.first().copied().unwrap_or(Value::Nil),
        _ => Value::Str(vm.heap.intern(b"error in error handling")),
    }
}
