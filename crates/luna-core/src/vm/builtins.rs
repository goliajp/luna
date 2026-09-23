//! Minimal base library — what the P03 gate corpus needs. The full base
//! library (P04) replaces/extends this.

use std::io::Write;

use crate::runtime::Value;
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
    let f = vm.native(crate::vm::lib_gc::nat_collectgarbage);
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
        let f = vm.native(nat_loadstring);
        vm.set_global("loadstring", f).expect("stdlib registration");
        let f = vm.native(crate::vm::lib_table::t_unpack);
        vm.set_global("unpack", f).expect("stdlib registration");
        // PUC 5.1 also exposed `gcinfo()` (memory in KB) and `newproxy()`.
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
        // `newproxy` remembers the metatables it created in a weak-keyed
        // set (PUC's upvalue `weaktable`): only a userdata carrying one of
        // them counts as a proxy whose metatable may be shared.
        let proxies = vm.heap.new_table();
        let weak_k = vm.heap.new_table();
        let mode_k = Value::Str(vm.heap.intern(b"__mode"));
        let mode_v = Value::Str(vm.heap.intern(b"k"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { weak_k.as_mut() }
            .set(&mut vm.heap, mode_k, mode_v)
            .expect("valid key");
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { proxies.as_mut() }.set_metatable(Some(weak_k));
        let f = vm.native_with(nat_newproxy, Box::new([Value::Table(proxies)]));
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
/// PUC `luaL_error`: a string message gets `luaL_where(L, 1)`, the position
/// of whatever called the running native — including a Lua frame that
/// reached it as a metamethod — and nothing when that caller is C.
fn raise(vm: &mut Vm, msg: Value) -> LuaError {
    match msg {
        Value::Str(s) => {
            let text = match vm.position_prefix_at_level(1) {
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
    raise_bytes(vm, msg.as_bytes())
}

/// `luaL_error` with a message that need not be UTF-8.
pub(crate) fn raise_bytes(vm: &mut Vm, msg: &[u8]) -> LuaError {
    let s = Value::Str(vm.heap.intern(msg));
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
            // `lua_call` from C: not yieldable.
            Some(ts) => match vm.call_noyield(ts, &[v]) {
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
            // `luaL_callmeta` is a plain `lua_call`: not yieldable.
            let r = vm.call_noyield(mm, &[v])?;
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
    // a bad key is the VM's error (`luaH_set` → `luaG_runerror`), raised
    // while rawset runs, so it carries no position
    vm.raw_set(t, k, v)?;
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
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
            // 5.2/5.3 call `__pairs` with a plain `lua_call` (not yieldable);
            // this path is only reached from C on 5.4+, where yielding is
            // impossible anyway.
            let res = vm.call_noyield(mm, &[t])?;
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
            // `lua_call`: not yieldable.
            let rs = vm.call_noyield(mm, &[t])?;
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
/// or by pcall, or through an unnamed expression — 5.2+ look the function up
/// by the name its library registered it under, and 5.1 prints '?'.
pub(crate) fn arg_error(vm: &mut Vm, n: u32, extra: &str) -> LuaError {
    // 5.5 counts the objects a `__call` chain put in front of the arguments
    // separately: an error in one of them is a "bad extra argument", and
    // the remaining arguments are numbered without them.
    let extraargs = if vm.version() >= LuaVersion::Lua55 {
        let ts = vm.thread_stack(None);
        if ts.levels.is_empty() {
            0
        } else {
            u32::try_from(vm.level_ar(&ts, 0).extraargs).expect("a __call chain length is small")
        }
    } else {
        0
    };
    let call_name = vm.running_call_name();
    let (argword, n) = if n <= extraargs {
        ("extra argument", n)
    } else {
        let n = n - extraargs;
        if let Some(("method", name)) = &call_name {
            let n = n - 1; // self is not counted
            if n == 0 {
                return raise_str(vm, &format!("calling '{name}' on bad self ({extra})"));
            }
            ("argument", n)
        } else {
            ("argument", n)
        }
    };
    let name = match call_name {
        Some((_, name)) => name,
        None => unnamed_native_name(vm),
    };
    raise_str(vm, &format!("bad {argword} #{n} to '{name}' ({extra})"))
}

/// `luaL_argerror`'s fallback when `ar.name` is NULL: '?' on 5.1, otherwise
/// the running native's library name, or '?'.
///
/// 5.2 finds the name by walking the global table, whose string hashes are
/// seeded per run, so it prints `'_G.tonumber'` on some runs and `'tonumber'`
/// on others (measured on stock 5.2.4). luna gives the short form, the one
/// 5.3 settled on.
fn unnamed_native_name(vm: &mut Vm) -> String {
    if vm.version() == crate::version::LuaVersion::Lua51 {
        return "?".to_string();
    }
    let Some(target) = vm.running_natives.last().map(|nc| nc.f) else {
        return "?".to_string();
    };
    vm.pushglobalfuncname(target)
        .unwrap_or_else(|| "?".to_string())
}

pub(crate) fn nat_tonumber(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() == LuaVersion::Lua51 {
        return tonumber_51(vm, a);
    }
    if a.is_none_or_nil(vm, 1) {
        let out = std_tonumber(vm, a)?;
        return Ok(vm.nat_return(fs, &[out]));
    }
    let out = if vm.version() == LuaVersion::Lua52 {
        // 5.2: the numeral may be given as a number; the base is a C int;
        // digits accumulate in a double.
        let s = argcheck::check_string(vm, a, 0)?;
        let base = argcheck::check_int(vm, a, 1)?;
        if !(2..=36).contains(&base) {
            return Err(arg_error(vm, 2, "base out of range"));
        }
        match str2number_base(s.as_bytes(), base as u32) {
            Some((neg, digits)) => {
                let n = digits.fold(0.0f64, |n, d| n * base as f64 + d as f64);
                Value::Float(if neg { -n } else { n })
            }
            None => Value::Nil,
        }
    } else {
        // 5.3+: the base is read first, the numeral must be a real string,
        // and digits accumulate in a wrapping unsigned integer.
        let base = argcheck::check_integer(vm, a, 1)?;
        let Value::Str(s) = a.get(vm, 0) else {
            return Err(argcheck::type_error(vm, a, 0, "string"));
        };
        if !(2..=36).contains(&base) {
            return Err(arg_error(vm, 2, "base out of range"));
        }
        match str2number_base(s.as_bytes(), base as u32) {
            Some((neg, digits)) => {
                let n = digits.fold(0u64, |n, d| {
                    n.wrapping_mul(base as u64).wrapping_add(d as u64)
                });
                Value::Int(if neg { n.wrapping_neg() } else { n } as i64)
            }
            None => Value::Nil,
        }
    };
    Ok(vm.nat_return(fs, &[out]))
}

/// `tonumber(v)` with no base on 5.2+: a number as is, a convertible string
/// converted, anything else nil — but an argument there must be.
fn std_tonumber(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    let v = a.get(vm, 0);
    match v {
        Value::Int(_) | Value::Float(_) => return Ok(v),
        Value::Str(_) => {
            if let Some(n) = argcheck::to_num(vm, v) {
                return Ok(match n {
                    crate::numeric::Num::Int(i) => Value::Int(i),
                    crate::numeric::Num::Float(f) => Value::Float(f),
                });
            }
        }
        _ => {}
    }
    argcheck::check_any(vm, a, 0)?;
    Ok(Value::Nil)
}

/// C `isspace` / the `SPACECHARS` set of 5.2+'s `tonumber`.
fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// The digit value of an ASCII alphanumeric in bases up to 36.
fn alnum_digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 10),
        b'A'..=b'Z' => Some((c - b'A') as u32 + 10),
        _ => None,
    }
}

/// 5.2+'s based numeral: optional spaces, an optional sign, a run of
/// alphanumerics that must all be digits of `base`, optional spaces, and
/// nothing else. Returns the sign and the digits.
fn str2number_base(s: &[u8], base: u32) -> Option<(bool, impl Iterator<Item = u32> + '_)> {
    let start = s.iter().position(|&c| !is_c_space(c)).unwrap_or(s.len());
    let mut rest = &s[start..];
    let neg = rest.first() == Some(&b'-');
    if matches!(rest.first(), Some(b'-' | b'+')) {
        rest = &rest[1..];
    }
    let n = rest
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    if n == 0 || rest[n..].iter().any(|&c| !is_c_space(c)) {
        return None;
    }
    let digits = &rest[..n];
    if digits
        .iter()
        .any(|&c| alnum_digit(c).is_none_or(|d| d >= base))
    {
        return None;
    }
    Some((
        neg,
        digits
            .iter()
            .map(|&c| alnum_digit(c).expect("checked above")),
    ))
}

/// 5.1 `luaB_tonumber`. Base 10 — given or defaulted — is the ordinary
/// conversion; any other base goes through C `strtoul`, whose result becomes
/// a double.
#[cold]
fn tonumber_51(vm: &mut Vm, a: Args) -> Result<u32, LuaError> {
    let base = argcheck::opt_int(vm, a, 1, 10)?;
    if base == 10 {
        argcheck::check_any(vm, a, 0)?;
        let out = match a.get(vm, 0) {
            v @ (Value::Int(_) | Value::Float(_)) => v,
            v @ Value::Str(_) => match argcheck::to_num(vm, v) {
                Some(n) => Value::Float(n.as_f64()),
                None => Value::Nil,
            },
            _ => Value::Nil,
        };
        return Ok(vm.nat_return(a.fs, &[out]));
    }
    let s = argcheck::check_string(vm, a, 0)?;
    if !(2..=36).contains(&base) {
        return Err(arg_error(vm, 2, "base out of range"));
    }
    let out = match strtoul(s.as_bytes(), base as u32) {
        Some(n) => Value::Float(n as f64),
        None => Value::Nil,
    };
    Ok(vm.nat_return(a.fs, &[out]))
}

/// What 5.1's `tonumber(s, base)` accepts: C `strtoul` on the string as a C
/// string (it ends at the first NUL), then only trailing spaces. `strtoul`
/// takes a sign — negating the unsigned value — and, in base 16, a `0x`
/// prefix; an out-of-range value is clamped to `ULONG_MAX`.
fn strtoul(s: &[u8], base: u32) -> Option<u64> {
    let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
    let mut i = s.iter().position(|&c| !is_c_space(c)).unwrap_or(s.len());
    let neg = s.get(i) == Some(&b'-');
    if matches!(s.get(i), Some(b'-' | b'+')) {
        i += 1;
    }
    if base == 16
        && s.get(i) == Some(&b'0')
        && matches!(s.get(i + 1), Some(b'x' | b'X'))
        && s.get(i + 2)
            .and_then(|&c| alnum_digit(c))
            .is_some_and(|d| d < 16)
    {
        i += 2;
    }
    let digits_start = i;
    let mut n: u64 = 0;
    let mut overflow = false;
    while let Some(d) = s.get(i).and_then(|&c| alnum_digit(c)).filter(|&d| d < base) {
        match n
            .checked_mul(base as u64)
            .and_then(|n| n.checked_add(d as u64))
        {
            Some(v) => n = v,
            None => overflow = true,
        }
        i += 1;
    }
    if i == digits_start || s[i..].iter().any(|&c| !is_c_space(c)) {
        return None;
    }
    Some(if overflow {
        u64::MAX
    } else if neg {
        n.wrapping_neg()
    } else {
        n
    })
}

/// `load`. 5.1 only loads from a reader function (`loadstring` takes the
/// strings); 5.2+ takes a string — or a number, which `lua_tolstring`
/// renders — or a reader, then an optional chunk name, mode and env.
pub(crate) fn nat_load(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() == LuaVersion::Lua51 {
        let name = argcheck::opt_string(vm, a, 1)?;
        let reader = argcheck::check_function(vm, a, 0)?;
        let name = name.map_or_else(|| b"=(load)".to_vec(), |n| n.as_bytes().to_vec());
        return match read_chunk(vm, reader)? {
            Ok(src) => load_chunk(vm, a, &src, &name, None),
            Err(msg) => Ok(vm.nat_return(fs, &[Value::Nil, msg])),
        };
    }
    let text = argcheck::to_str_bytes(vm, a.get(vm, 0));
    let mode = load_mode(vm, a, 2)?;
    let (src, name) = match text {
        Some(src) => {
            let name = argcheck::opt_string(vm, a, 1)?;
            let name = name.map_or_else(|| src.clone(), |n| n.as_bytes().to_vec());
            (src, name)
        }
        None => {
            let name = argcheck::opt_string(vm, a, 1)?;
            let name = name.map_or_else(|| b"=(load)".to_vec(), |n| n.as_bytes().to_vec());
            let reader = argcheck::check_function(vm, a, 0)?;
            match read_chunk(vm, reader)? {
                Ok(src) => (src, name),
                Err(msg) => return Ok(vm.nat_return(fs, &[Value::Nil, msg])),
            }
        }
    };
    load_chunk(vm, a, &src, &name, mode.as_deref())
}

/// 5.1 `loadstring(s [, chunkname])`: the source is a string (or a number,
/// rendered), and the chunk name defaults to the source itself.
fn nat_loadstring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let src = argcheck::check_string(vm, a, 0)?;
    let name = argcheck::opt_string(vm, a, 1)?.unwrap_or(src);
    load_chunk(vm, a, src.as_bytes(), name.as_bytes(), None)
}

/// The `mode` argument of 5.2+ `load`: `luaL_optstring` with the dialect's
/// default ("bt" up to 5.4, none on 5.5, where an absent mode allows both).
/// 5.5 refuses 'B' (a fixed-buffer chunk, which Lua code cannot supply).
fn load_mode(vm: &mut Vm, a: Args, i: u32) -> Result<Option<Vec<u8>>, LuaError> {
    let mode = argcheck::opt_string(vm, a, i)?.map(|m| m.as_bytes().to_vec());
    if vm.version() >= LuaVersion::Lua55 {
        if mode.as_ref().is_some_and(|m| m.contains(&b'B')) {
            return Err(arg_error(vm, i + 1, "invalid mode"));
        }
        return Ok(mode);
    }
    Ok(Some(mode.unwrap_or_else(|| b"bt".to_vec())))
}

/// Drain a `load` reader: it is called until it returns nil, no value or
/// the empty string. A string or number piece is appended; any other value,
/// or an error the reader raises, fails the load softly — `Ok(Err(msg))`
/// is the message `load` returns after its nil.
fn read_chunk(vm: &mut Vm, reader: Value) -> Result<Result<Vec<u8>, Value>, LuaError> {
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
        // the parser runs with the thread non-yieldable (`nny` is raised
        // around `lua_load`), so the reader cannot yield
        let r = match vm.call_noyield(reader, &[]) {
            Ok(r) => r,
            Err(e) => return Ok(Err(e.0)),
        };
        let piece = match r.first() {
            None | Some(Value::Nil) => break,
            Some(&v) => match argcheck::to_str_bytes(vm, v) {
                Some(b) => b,
                None => {
                    let m = Value::Str(vm.heap.intern(b"reader function must return a string"));
                    return Ok(Err(m));
                }
            },
        };
        if piece.is_empty() {
            break;
        }
        // Gate the next chunk *before* we extend the
        // buffer: PUC's `loadrep` feeder returns a 1 MiB
        // string every iteration and runs forever; with
        // the default 256 MiB cap we error out after the
        // first quarter-gig instead of letting the host
        // allocator crawl past 7 GB then SIGSEGV.
        // Matches the PUC `not enough memory` failure
        // shape that `heavy.lua::loadrep` asserts on.
        if piece.len() > input_budget.saturating_sub(buf.len()) {
            return Ok(Err(Value::Str(vm.heap.intern(b"not enough memory"))));
        }
        buf.extend_from_slice(&piece);
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
    Ok(Ok(buf))
}

/// Compile `src` as chunk `name` and return `load`'s results: the function,
/// or nil and the message. `mode` restricts text/binary chunks
/// (`checkmode` in ldo.c); a 4th argument, when present, becomes the
/// function's first upvalue (5.2+ `load`).
fn load_chunk(
    vm: &mut Vm,
    a: Args,
    src: &[u8],
    name: &[u8],
    mode: Option<&[u8]>,
) -> Result<u32, LuaError> {
    let binary = crate::vm::dump::is_binary_chunk(src);
    let kind: &[u8] = if binary { b"b" } else { b"t" };
    // `strchr`: the mode is a C string, so it ends at the first NUL.
    if let Some(mode) = mode.map(|m| &m[..m.iter().position(|&c| c == 0).unwrap_or(m.len())])
        && !mode.contains(&kind[0])
    {
        let msg = format!(
            "attempt to load a {} chunk (mode is '{}')",
            if binary { "binary" } else { "text" },
            String::from_utf8_lossy(mode)
        );
        let m = Value::Str(vm.heap.intern(msg.as_bytes()));
        return Ok(vm.nat_return(a.fs, &[Value::Nil, m]));
    }
    match vm.load(src, name) {
        Ok(cl) => {
            // `lua_setupvalue(L, -2, 1)`: a function without upvalues
            // ignores the env.
            if vm.version() >= LuaVersion::Lua52 && !a.is_none(3) && !cl.upvals().is_empty() {
                let env = a.get(vm, 3);
                let uv = vm.heap.new_upvalue(crate::runtime::UpvalState::Closed(env));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { cl.as_mut() }.upvals_mut()[0] = uv;
            }
            Ok(vm.nat_return(a.fs, &[Value::Closure(cl)]))
        }
        Err(e) => {
            // PUC formats the syntax error's source prefix via `luaO_chunkid`
            // (see `syntax_chunk_id`), not as a bare `[string "<name>"]`. This handles
            // the `@file` / `=name` sigils and head/tail-truncation rules.
            // `e.msg` carries raw bytes (PUC's near-token may be a non-UTF-8
            // byte from the source) — splice it in as-is so 5.1 errors.lua
            // can pattern-match `near '\xff'` etc.
            let display = crate::vm::callstack::syntax_chunk_id(vm.version(), name);
            let m = Value::Str(vm.heap.intern(&e.render(&display)));
            Ok(vm.nat_return(a.fs, &[Value::Nil, m]))
        }
    }
}

/// PUC 5.1 `newproxy([false | true | proxy])`: an empty userdata, with no
/// metatable, a fresh one, or the metatable of another proxy. Only a
/// userdata whose metatable `newproxy(true)` created — recorded in the weak
/// set held as upvalue 0 — is a proxy; anything else is an argument error.
fn nat_newproxy(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::userdata::UserdataPayload;
    let Value::Table(proxies) = vm.nat_upval(fs, 0) else {
        unreachable!("newproxy is registered with its metatable set")
    };
    let arg = vm.nat_arg(fs, nargs, 0);
    let mt = match arg {
        v if !v.truthy() => None,
        Value::Bool(true) => {
            let m = vm.heap.new_table();
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { proxies.as_mut() }
                .set(&mut vm.heap, Value::Table(m), Value::Bool(true))
                .expect("a table key is never nil or NaN");
            vm.barrier_back_table(proxies);
            Some(m)
        }
        v => match vm.metatable_of(v) {
            Some(m) if proxies.get(Value::Table(m)).truthy() => Some(m),
            _ => return Err(arg_error(vm, 1, "boolean or proxy expected")),
        },
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

/// What PUC 5.1's `getfunc` finds for argument 1 of `getfenv`/`setfenv`.
enum FenvTarget {
    /// A Lua function: given directly, or running at the level.
    Lua(crate::runtime::Gc<crate::runtime::LuaClosure>),
    /// A C function, given directly or running at the level (level 0 is
    /// the calling `getfenv`/`setfenv` itself).
    C,
}

/// PUC 5.1 `getfunc`: argument 1 is a function, or a stack level — optional
/// (default 1) for `getfenv`, required for `setfenv`.
fn fenv_target(vm: &mut Vm, a: Args, level_optional: bool) -> Result<FenvTarget, LuaError> {
    match a.get(vm, 0) {
        Value::Closure(c) => return Ok(FenvTarget::Lua(c)),
        Value::Native(_) => return Ok(FenvTarget::C),
        _ => {}
    }
    let level = if level_optional {
        argcheck::opt_int(vm, a, 0, 1)?
    } else {
        argcheck::check_int(vm, a, 0)?
    };
    if level < 0 {
        return Err(arg_error(vm, 1, "level must be non-negative"));
    }
    if level == 0 {
        return Ok(FenvTarget::C);
    }
    use crate::vm::callstack::DbgKind;
    match vm.dbg_frame(level as i64) {
        Some(DbgKind::Lua(_)) => Ok(FenvTarget::Lua(
            vm.lua_closure_at_level(level as i64)
                .expect("a Lua level has a closure"),
        )),
        Some(DbgKind::C(_)) => Ok(FenvTarget::C),
        Some(DbgKind::Tail) => Err(raise_str(
            vm,
            &format!("no function environment for tail call at level {level}"),
        )),
        None => Err(arg_error(vm, 1, "invalid level")),
    }
}

/// The index of the `_ENV` upvalue of a 5.1 closure. It is *not* guaranteed
/// to sit at slot 0 — closures capture upvalues in first-access order, so a
/// body that touches a local upvalue (e.g. `local saved = print;
/// saved("hi"); module(...)`) puts the local ahead of `_ENV`.
fn env_upvalue(cl: crate::runtime::Gc<crate::runtime::LuaClosure>) -> Option<usize> {
    cl.proto.upvals.iter().position(|d| &*d.name == "_ENV")
}

/// PUC 5.1 `setfenv(f|level, env)`: replace the env of the Lua function `f`
/// (or of the Lua function at stack `level`). Writes through the closure's
/// `_ENV` cell (which the 5.1 `Op::Closure` path keeps per-closure, so the
/// rewrite only affects this specific function — not its siblings).
fn nat_setfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let env = argcheck::check_table(vm, a, 1)?;
    let target = fenv_target(vm, a, false)?;
    // `setfenv(0, env)` (a numeric 0, string "0" included) rewrites the
    // thread's global table (`L->l_gt`). luna repoints `Vm.globals` so future
    // `vm.load` calls snapshot the new table into the loaded chunk's `_ENV`
    // cell; already-loaded closures keep their own per-closure `_ENV` cell.
    // locals.lua's `foo("")` probe relies on the loaded-chunk path picking up
    // the new table.
    if argcheck::to_num(vm, a.get(vm, 0)).is_some_and(|n| n.as_f64() == 0.0) {
        vm.set_globals(env);
        return Ok(vm.nat_return(fs, &[]));
    }
    let cl = match target {
        FenvTarget::Lua(cl) => cl,
        FenvTarget::C => {
            return Err(raise_str(
                vm,
                "'setfenv' cannot change environment of given object",
            ));
        }
    };
    let env_idx = env_upvalue(cl)
        .ok_or_else(|| raise_str(vm, "'setfenv' cannot change environment of given object"))?;
    let uv = cl.upvals()[env_idx];
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { uv.as_mut() }.set_closed(Value::Table(env));
    vm.barrier_forward_upvalue(uv, Value::Table(env));
    Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
}

/// PUC 5.1 `getfenv(f|level)`: the env of the Lua function `f` (or of the
/// Lua function at stack `level`); for a C function, the thread's globals.
fn nat_getfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::UpvalState;
    let env = match fenv_target(vm, Args::new(fs, nargs), true)? {
        FenvTarget::Lua(c) => match env_upvalue(c) {
            Some(i) => match c.upvals()[i].state() {
                UpvalState::Closed(v) => v,
                UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
            },
            None => Value::Table(vm.globals()),
        },
        FenvTarget::C => Value::Table(vm.globals()),
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

/// `xpcall` reached through `call_value`; a call from Lua goes through
/// `Vm::begin_xpcall` instead, which makes the protected call yieldable.
pub(crate) fn nat_xpcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.1 `xpcall(f, err)` calls `f` with no arguments.
    xpcall_native(vm, fs, nargs, vm.version() > LuaVersion::Lua51)
}

/// The C level of a host's protected call (`Vm::call_value_with_handler`):
/// an `xpcall` that passes its extra arguments on in every dialect, as
/// `lua_pcall` does. The dispatcher runs it as it runs `xpcall`; this body
/// only runs for a call that reaches it through `call_value`.
pub(crate) fn nat_host_xpcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    xpcall_native(vm, fs, nargs, true)
}

fn xpcall_native(vm: &mut Vm, fs: u32, nargs: u32, forward: bool) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let h = xpcall_handler(vm, a)?;
    let f = vm.nat_arg(fs, nargs, 0);
    let first_arg = if forward { 2 } else { nargs };
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
