//! The debug library (PUC `ldblib.c`), per dialect. Stack levels,
//! function info and local variables come from the call-stack view in
//! `callstack`; this module is the argument handling and result shaping of
//! each `db_*` function.

use crate::runtime::{Coro, Gc, Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_any, check_integer, opt_integer, type_error};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) use crate::vm::callstack::chunk_id;

mod hooks;
mod info;
mod locals;
mod traceback;
mod upvalues;
use hooks::{d_gethook, d_sethook};
pub(crate) use info::activelines;
use info::d_getinfo;
pub(crate) use locals::param_name;
use locals::{d_getlocal, d_setlocal};
use traceback::d_traceback;
use upvalues::{d_getupvalue, d_setupvalue, d_upvalueid, d_upvaluejoin};
pub(crate) use upvalues::{upvalue_name, visible_upvalue_index};

pub(crate) fn open_debug(vm: &mut Vm) {
    let v = vm.version();
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: `t` is the table allocated above, so it is alive; no reference into it is held across this call, and `set` does not collect
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    set(vm, "debug", d_debug);
    set(vm, "gethook", d_gethook);
    set(vm, "getinfo", d_getinfo);
    set(vm, "getlocal", d_getlocal);
    set(vm, "getregistry", d_getregistry);
    set(vm, "getmetatable", d_getmetatable);
    set(vm, "getupvalue", d_getupvalue);
    set(vm, "sethook", d_sethook);
    set(vm, "setlocal", d_setlocal);
    set(vm, "setmetatable", d_setmetatable);
    set(vm, "setupvalue", d_setupvalue);
    set(vm, "traceback", d_traceback);
    if v == LuaVersion::Lua51 {
        set(vm, "getfenv", d_getfenv);
        set(vm, "setfenv", d_setfenv);
    } else {
        set(vm, "getuservalue", d_getuservalue);
        set(vm, "setuservalue", d_setuservalue);
        set(vm, "upvalueid", d_upvalueid);
        set(vm, "upvaluejoin", d_upvaluejoin);
    }
    if v == LuaVersion::Lua54 {
        set(vm, "setcstacklimit", d_setcstacklimit);
    }
    vm.set_global("debug", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
    // PUC's LUA_REGISTRYINDEX table — eagerly built so `_HOOKKEY` (weak-key)
    // is observable from db.lua :328 the moment the debug library loads.
    init_registry(vm);
    // register in package.loaded so require"debug" finds it
    let pkg_k = Value::Str(vm.heap.intern(b"package"));
    if let Value::Table(pkg) = vm.globals().get(pkg_k) {
        let lk = Value::Str(vm.heap.intern(b"loaded"));
        if let Value::Table(loaded) = pkg.get(lk) {
            let dk = Value::Str(vm.heap.intern(b"debug"));
            // SAFETY: `loaded` was just read from `package.loaded`, so it is alive; no reference into it is held across this call, and `set` does not collect
            unsafe { loaded.as_mut() }
                .set(&mut vm.heap, dk, Value::Table(t))
                .expect("valid key");
            vm.barrier_back_table(loaded);
        }
    }
}

fn set_field(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let key = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: `t` is a table the caller allocated or holds in a local, so it is alive; no reference into it is held across this call, and `set` does not collect
    unsafe { t.as_mut() }
        .set(&mut vm.heap, key, v)
        .expect("valid key");
}

fn set_str(vm: &mut Vm, t: Gc<Table>, k: &str, bytes: &[u8]) {
    let v = Value::Str(vm.heap.intern(bytes));
    set_field(vm, t, k, v);
}

/// Build PUC's `LUA_REGISTRYINDEX` table (kept on `Vm.registry`, a GC root)
/// and populate `_HOOKKEY` (PUC `db_sethook`'s per-thread weak-key table).
/// db.lua :328 only checks `__mode == 'k'`; luna's sethook stores hook state
/// directly in `Vm.hook`/`Coro.hook`, so the entry is shape-only.
fn init_registry(vm: &mut Vm) {
    // the C API may have made the registry already
    let reg = vm.host_registry();
    let hook_t = vm.heap.new_table();
    let mt = vm.heap.new_table();
    let mode_k = Value::Str(vm.heap.intern(b"k"));
    set_field(vm, mt, "__mode", mode_k);
    vm.barrier_back_table(mt);
    // SAFETY: `hook_t` was allocated above and is held only by this local; the borrow covers one call
    unsafe { hook_t.as_mut() }.set_metatable(Some(mt));
    vm.barrier_back_table(hook_t);
    set_field(vm, reg, "_HOOKKEY", Value::Table(hook_t));
    if vm.ignore_env && vm.version() >= LuaVersion::Lua52 {
        set_field(vm, reg, "LUA_NOENV", Value::Bool(true));
    }
    vm.barrier_back_table(reg);
}

/// PUC `getthread`: an optional leading thread argument, and the offset of
/// the arguments after it.
fn getthread(vm: &Vm, a: Args) -> (Option<Gc<Coro>>, u32) {
    match a.get(vm, 0) {
        Value::Coro(co) if !a.is_none(0) => (Some(co), 1),
        _ => (None, 0),
    }
}

/// `(int)luaL_checkinteger` — levels and indices are C ints.
fn check_int(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    Ok(check_integer(vm, a, i)? as i32 as i64)
}

fn is_function(vm: &Vm, a: Args, i: u32) -> bool {
    !a.is_none(i) && matches!(a.get(vm, i), Value::Closure(_) | Value::Native(_))
}

fn d_getregistry(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let r = vm.registry.map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[r]))
}

fn d_getmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = check_any(vm, Args::new(fs, nargs), 0)?;
    let mt = vm.metatable_of(v).map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[mt]))
}

fn d_setmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = a.get(vm, 0);
    let m = match a.get(vm, 1) {
        Value::Nil if !a.is_none(1) => None,
        Value::Table(m) => Some(m),
        _ if vm.version() >= LuaVersion::Lua54 => return Err(type_error(vm, a, 1, "nil or table")),
        _ => return Err(arg_error(vm, 2, "nil or table expected")),
    };
    match v {
        Value::Table(t) => {
            // Redis's `lua_setmetatable` refuses a read-only table
            vm.refuse_readonly(t)?;
            // SAFETY: `t` is the first argument, kept alive by its stack slot; the borrow covers one call, and `m` is a separate handle, not a reference into `t`
            unsafe { t.as_mut() }.set_metatable(m);
            vm.barrier_back_table(t);
        }
        Value::Userdata(u) => {
            // SAFETY: `u` is the first argument, kept alive by its stack slot; the borrow covers one call
            unsafe { u.as_mut() }.set_metatable(m);
            vm.heap.barrier_back(u);
        }
        // every other type shares one metatable per basic type
        _ => vm.set_type_metatable(v, m),
    }
    // 5.1 returns `lua_setmetatable`'s status, always 1
    let r = if vm.version() == LuaVersion::Lua51 {
        Value::Bool(true)
    } else {
        v
    };
    Ok(vm.nat_return(fs, &[r]))
}

fn d_getuservalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() <= LuaVersion::Lua53 {
        let r = match a.get(vm, 0) {
            Value::Userdata(u) if !a.is_none(0) => vm.host_uservalue(u, 1).unwrap_or(Value::Nil),
            _ => Value::Nil,
        };
        return Ok(vm.nat_return(fs, &[r]));
    }
    // 5.4+: user value `n` of a full userdata and `true`, or nil alone when
    // it has no such value
    let n = opt_integer(vm, a, 1, 1)? as i32;
    let found = match a.get(vm, 0) {
        Value::Userdata(u) if !a.is_none(0) => usize::try_from(n)
            .ok()
            .and_then(|n| vm.host_uservalue(u, n)),
        _ => None,
    };
    match found {
        Some(v) => Ok(vm.nat_return(fs, &[v, Value::Bool(true)])),
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

fn d_setuservalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let n = if v >= LuaVersion::Lua54 {
        opt_integer(vm, a, 2, 1)? as i32
    } else {
        1
    };
    if v == LuaVersion::Lua52 && matches!(a.get(vm, 0), Value::LightUserdata(_)) && !a.is_none(0) {
        return Err(arg_error(
            vm,
            1,
            "full userdata expected, got light userdata",
        ));
    }
    let u = match a.get(vm, 0) {
        Value::Userdata(u) if !a.is_none(0) => u,
        _ => return Err(type_error(vm, a, 0, "userdata")),
    };
    let value = if v == LuaVersion::Lua52 {
        match a.get(vm, 1) {
            _ if a.is_none_or_nil(vm, 1) => Value::Nil,
            t @ Value::Table(_) => t,
            _ => return Err(type_error(vm, a, 1, "table")),
        }
    } else {
        check_any(vm, a, 1)?
    };
    let set = usize::try_from(n).is_ok_and(|n| vm.host_set_uservalue(u, n, value));
    if !set {
        // `lua_setiuservalue` failed: no such user value
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// PUC `db_debug`: read commands from stdin (in 250-byte `fgets` chunks),
/// prompting on stderr, until end of input or a line that is exactly
/// `cont`; run each as a protected chunk and print its error.
fn d_debug(vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::io::{BufRead, Write};
    loop {
        eprint!("lua_debug> ");
        let _ = std::io::stderr().flush(); // stderr is unbuffered; nothing to report
        let mut line = Vec::new();
        let mut stdin = std::io::stdin().lock();
        while line.len() < 249 {
            let buf = match stdin.fill_buf() {
                Ok(b) => b,
                Err(_) => break,
            };
            let Some(&c) = buf.first() else { break };
            stdin.consume(1);
            line.push(c);
            if c == b'\n' {
                break;
            }
        }
        drop(stdin);
        if line.is_empty() || line == b"cont\n" {
            return Ok(0);
        }
        if let Err(msg) = run_debug_command(vm, &line) {
            eprintln!("{}", String::from_utf8_lossy(&msg));
        }
    }
}

/// Load and call one `debug.debug` command, returning the error message to
/// print when it fails.
fn run_debug_command(vm: &mut Vm, line: &[u8]) -> Result<(), Vec<u8>> {
    let v = vm.version();
    if v >= LuaVersion::Lua55 && crate::vm::dump::is_binary_chunk(line) {
        return Err(b"attempt to load a binary chunk (mode is 't')".to_vec());
    }
    // `luaL_loadbuffer` parses under the running message handler (5.4+
    // raise a too-deep command's "C stack overflow" through it); the
    // command itself runs under `lua_pcall` without one
    let err = match vm.load(line, b"=(debug command)") {
        Ok(f) => match vm.call_protected(Value::Closure(f), &[]) {
            Ok(_) => return Ok(()),
            Err(e) => e.0,
        },
        Err(e) => vm.load_error_value(&e, b"=(debug command)"),
    };
    Err(match err {
        Value::Str(s) => s.as_bytes().to_vec(),
        n @ (Value::Int(_) | Value::Float(_)) => {
            crate::vm::argcheck::to_str_bytes(vm, n).expect("a number")
        }
        // 5.4+ print any value with `luaL_tolstring`; earlier versions pass
        // `lua_tostring`'s NULL to printf
        other if v >= LuaVersion::Lua54 => vm
            .tostring_value(other)
            .unwrap_or_else(|e| vm.error_text(&e).into_bytes()),
        _ => b"(null)".to_vec(),
    })
}

fn d_setcstacklimit(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    check_int(vm, Args::new(fs, nargs), 0)?;
    // PUC 5.4.9's `lua_setcstacklimit` is a stub that returns LUAI_MAXCCALLS
    Ok(vm.nat_return(fs, &[Value::Int(200)]))
}

/// PUC 5.1 `debug.setfenv(o, env)`: replace the environment of a function,
/// thread or userdata. Returns `o`.
fn d_setfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let env_t = crate::vm::argcheck::check_table(vm, a, 1)?;
    let o = a.get(vm, 0);
    match o {
        Value::Closure(cl) => {
            // the closure's `_ENV` cell stands for its 5.1 environment
            let Some(env_idx) = cl.proto.upvals.iter().position(|d| &*d.name == "_ENV") else {
                return Err(raise_str(
                    vm,
                    "'setfenv' cannot change environment of given object",
                ));
            };
            vm.set_closure_env(cl, env_idx, env_t);
        }
        Value::Coro(co) => {
            // the running thread's globals are the Vm's; a suspended one
            // keeps its own until resumed
            if vm.is_current_thread(Some(co)) {
                vm.set_globals(env_t);
            } else {
                // SAFETY: `co` is a native argument (kept alive by its stack slot) and not the running thread, so nothing in the Vm refers into its saved state; the borrow covers one field store
                unsafe { co.as_mut() }.globals = env_t;
                vm.heap.barrier_back(co);
            }
        }
        // a userdata the C API made keeps its environment as its user
        // value; luna keeps none on natives or other userdata, where PUC's
        // change would be observable only through `getfenv` of that object
        Value::Userdata(u) if vm.host_block(u).is_some() => {
            vm.host_set_uservalue(u, 1, Value::Table(env_t));
        }
        Value::Native(_) | Value::Userdata(_) => {}
        _ => {
            return Err(raise_str(
                vm,
                "'setfenv' cannot change environment of given object",
            ));
        }
    }
    Ok(vm.nat_return(fs, &[o]))
}

/// PUC 5.1 `debug.getfenv(o)`: the environment of a function, thread or
/// userdata; nil for any other value.
fn d_getfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::UpvalState;
    let o = check_any(vm, Args::new(fs, nargs), 0)?;
    let env = match o {
        Value::Closure(cl) => match cl.proto.upvals.iter().position(|d| &*d.name == "_ENV") {
            Some(i) => match cl.upvals()[i].state() {
                UpvalState::Closed(v) => v,
                UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
            },
            None => Value::Table(vm.globals()),
        },
        Value::Coro(co) if !vm.is_current_thread(Some(co)) => Value::Table(co.globals),
        Value::Userdata(u) if vm.host_block(u).is_some() => {
            vm.host_uservalue(u, 1).unwrap_or(Value::Nil)
        }
        Value::Coro(_) | Value::Native(_) | Value::Userdata(_) => Value::Table(vm.globals()),
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[env]))
}
