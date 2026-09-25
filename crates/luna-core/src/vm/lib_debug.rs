//! The debug library (PUC `ldblib.c`), per dialect. Stack levels,
//! function info and local variables come from the call-stack view in
//! `callstack`; this file is the argument handling and result shaping of
//! each `db_*` function.

use crate::runtime::{Coro, Gc, LuaClosure, Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{
    Args, check_any, check_function, check_integer, check_string, opt_integer, opt_string, to_num,
    type_error,
};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::callstack::{Ar, DbgKind, LocalSlot};
use crate::vm::error::LuaError;
use crate::vm::exec::{HookState, Vm};

pub(crate) use crate::vm::callstack::chunk_id;

pub(crate) fn open_debug(vm: &mut Vm) {
    let v = vm.version();
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
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
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { loaded.as_mut() }
                .set(&mut vm.heap, dk, Value::Table(t))
                .expect("valid key");
            vm.barrier_back_table(loaded);
        }
    }
}

fn set_field(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let key = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
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
    let reg = vm.heap.new_table();
    let hook_t = vm.heap.new_table();
    let mt = vm.heap.new_table();
    let mode_k = Value::Str(vm.heap.intern(b"k"));
    set_field(vm, mt, "__mode", mode_k);
    vm.barrier_back_table(mt);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { hook_t.as_mut() }.set_metatable(Some(mt));
    vm.barrier_back_table(hook_t);
    set_field(vm, reg, "_HOOKKEY", Value::Table(hook_t));
    vm.barrier_back_table(reg);
    vm.registry = Some(reg);
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
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { t.as_mut() }.set_metatable(m);
            vm.barrier_back_table(t);
        }
        Value::Userdata(u) => {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { u.as_mut() }.set_metatable(m);
            vm.heap
                .barrier_back(u.as_ptr() as *mut crate::runtime::heap::GcHeader);
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

/// The option letters `lua_getinfo` accepts in each version.
fn valid_options(v: LuaVersion) -> &'static [u8] {
    match v {
        LuaVersion::Lua51 => b"SlunLf",
        LuaVersion::Lua52 | LuaVersion::Lua53 => b"SlutnLf",
        _ => b"SlutnrLf",
    }
}

fn d_getinfo(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let (co, arg) = getthread(vm, a);
    let options: Vec<u8> = match opt_string(vm, a, arg + 1)? {
        Some(s) => s.as_bytes().to_vec(),
        None => match v {
            LuaVersion::Lua51 => b"flnSu".to_vec(),
            LuaVersion::Lua52 | LuaVersion::Lua53 => b"flnStu".to_vec(),
            _ => b"flnSrtu".to_vec(),
        },
    };
    if v >= LuaVersion::Lua54 && options.first() == Some(&b'>') {
        return Err(arg_error(vm, arg + 2, "invalid option '>'"));
    }
    let subject = a.get(vm, arg);
    let level = if v <= LuaVersion::Lua52 {
        // `lua_isnumber` comes first, so a numeric string is a level
        match to_num(vm, subject) {
            Some(n) if !a.is_none(arg) => Some(match n {
                crate::numeric::Num::Int(i) => i,
                crate::numeric::Num::Float(f) => f as i64,
            } as i32 as i64),
            _ if is_function(vm, a, arg) => None,
            _ => return Err(arg_error(vm, arg + 1, "function or level expected")),
        }
    } else if is_function(vm, a, arg) {
        None
    } else {
        Some(check_int(vm, a, arg)?)
    };
    let (ar, activelines) = match level {
        None => {
            let ar = vm.function_ar(subject);
            let lines = activelines(vm, subject);
            (ar, lines)
        }
        Some(level) => {
            let ts = vm.thread_stack(co);
            let found = if level < 0 {
                // 5.1's `lua_getstack` reports a negative level as a lost
                // tail call
                (v == LuaVersion::Lua51).then_some(None)
            } else {
                ts.levels
                    .get(level as usize)
                    .map(|&k| Some((level as usize, k)))
            };
            match found {
                None => {
                    drop(ts);
                    return Ok(vm.nat_return(fs, &[Value::Nil]));
                }
                Some(None) | Some(Some((_, DbgKind::Tail))) => {
                    // PUC 5.1 `auxgetinfo` returns before looking at the
                    // options of a lost tail call
                    drop(ts);
                    let ar = crate::vm::callstack::tail_ar();
                    let t = info_table(vm, &options, &ar, Value::Nil);
                    return Ok(vm.nat_return(fs, &[Value::Table(t)]));
                }
                Some(Some((i, _))) => {
                    let ar = vm.level_ar(&ts, i);
                    drop(ts);
                    let lines = activelines(vm, ar.func);
                    (ar, lines)
                }
            }
        }
    };
    let allowed = valid_options(v);
    if options.iter().any(|c| !allowed.contains(c)) {
        return Err(arg_error(vm, arg + 2, "invalid option"));
    }
    let t = info_table(vm, &options, &ar, activelines);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

/// PUC `collectvalidlines`: the lines holding an instruction, as a set; nil
/// for a C function.
fn activelines(vm: &mut Vm, f: Value) -> Value {
    let Value::Closure(cl) = f else {
        return Value::Nil;
    };
    let lines = vm.heap.new_table();
    for &ln in cl.proto.lines.iter() {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { lines.as_mut() }
            .set(&mut vm.heap, Value::Int(ln as i64), Value::Bool(true))
            .expect("valid line key");
    }
    vm.barrier_back_table(lines);
    Value::Table(lines)
}

/// `db_getinfo`'s result table: the fields of each requested option.
fn info_table(vm: &mut Vm, options: &[u8], ar: &Ar, activelines: Value) -> Gc<Table> {
    let v = vm.version();
    let t = vm.heap.new_table();
    let has = |c: u8| options.contains(&c);
    if has(b'S') {
        // ≤5.3 push the source as a C string (up to a NUL); 5.4+ keep its
        // length. `short_src` is always a C string.
        let source = if v <= LuaVersion::Lua53 {
            cstr(&ar.source)
        } else {
            &ar.source[..]
        };
        set_str(vm, t, "source", source);
        set_str(vm, t, "short_src", cstr(&ar.short_src));
        set_field(vm, t, "linedefined", Value::Int(ar.linedefined));
        set_field(vm, t, "lastlinedefined", Value::Int(ar.lastlinedefined));
        set_str(vm, t, "what", ar.what.as_bytes());
    }
    if has(b'l') {
        set_field(vm, t, "currentline", Value::Int(ar.currentline));
    }
    if has(b'u') {
        set_field(vm, t, "nups", Value::Int(ar.nups));
        if v >= LuaVersion::Lua52 {
            set_field(vm, t, "nparams", Value::Int(ar.nparams));
            set_field(vm, t, "isvararg", Value::Bool(ar.isvararg));
        }
    }
    if has(b'n') {
        match &ar.name {
            Some((what, name)) => {
                set_str(vm, t, "name", name.as_bytes());
                set_str(vm, t, "namewhat", what.as_bytes());
            }
            None => set_str(vm, t, "namewhat", b""),
        }
    }
    if has(b'r') {
        set_field(vm, t, "ftransfer", Value::Int(ar.ftransfer));
        set_field(vm, t, "ntransfer", Value::Int(ar.ntransfer));
    }
    if has(b't') {
        set_field(vm, t, "istailcall", Value::Bool(ar.istailcall));
        if v >= LuaVersion::Lua55 {
            set_field(vm, t, "extraargs", Value::Int(ar.extraargs));
        }
    }
    if has(b'L') {
        set_field(vm, t, "activelines", activelines);
    }
    if has(b'f') {
        set_field(vm, t, "func", ar.func);
    }
    vm.barrier_back_table(t);
    t
}

fn cstr(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(n) => &b[..n],
        None => b,
    }
}

/// `lua_getstack` for the local-variable functions: the level index
/// (`None`: 5.1's answer to a negative level, a lost tail call, which has
/// no locals), or the "level out of range" argument error.
fn stack_level(
    vm: &mut Vm,
    co: Option<Gc<Coro>>,
    level: i64,
    argn: u32,
) -> Result<Option<usize>, LuaError> {
    let n = vm.thread_stack(co).levels.len();
    match usize::try_from(level) {
        Ok(i) if i < n => Ok(Some(i)),
        _ if level < 0 && vm.version() == LuaVersion::Lua51 => Ok(None),
        _ => Err(arg_error(vm, argn, "level out of range")),
    }
}

fn read_local(vm: &Vm, co: Option<Gc<Coro>>, at: LocalSlot) -> Value {
    match at {
        LocalSlot::Held(v) => v,
        LocalSlot::Stack(slot) => {
            let ts = vm.thread_stack(co);
            // luna sizes the value stack lazily; a slot past it holds nil
            ts.stack.get(slot).copied().unwrap_or(Value::Nil)
        }
    }
}

fn find_local(
    vm: &Vm,
    co: Option<Gc<Coro>>,
    level: Option<usize>,
    n: i64,
) -> Option<(String, LocalSlot)> {
    let ts = vm.thread_stack(co);
    vm.find_local(&ts, level?, n)
}

fn d_getlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let (co, arg) = getthread(vm, a);
    if v == LuaVersion::Lua51 {
        let level = check_int(vm, a, arg)?;
        let level = stack_level(vm, co, level, arg + 1)?;
        let n = check_int(vm, a, arg + 1)?;
        return getlocal_result(vm, fs, co, level, n);
    }
    let n = check_int(vm, a, arg + 1)?;
    if is_function(vm, a, arg) {
        // the name of parameter `n`, from the prototype
        let name = match a.get(vm, arg) {
            Value::Closure(cl) => param_name(cl, n),
            _ => None,
        };
        let r = match name {
            Some(nm) => Value::Str(vm.heap.intern(nm.as_bytes())),
            None => Value::Nil,
        };
        return Ok(vm.nat_return(fs, &[r]));
    }
    let level = check_int(vm, a, arg)?;
    let level = stack_level(vm, co, level, arg + 1)?;
    getlocal_result(vm, fs, co, level, n)
}

fn getlocal_result(
    vm: &mut Vm,
    fs: u32,
    co: Option<Gc<Coro>>,
    level: Option<usize>,
    n: i64,
) -> Result<u32, LuaError> {
    match find_local(vm, co, level, n) {
        Some((name, at)) => {
            let val = read_local(vm, co, at);
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm, val]))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

/// PUC `luaF_getlocalname(p, n, 0)`: the `n`-th local live at the first
/// instruction — a parameter.
fn param_name(cl: Gc<LuaClosure>, n: i64) -> Option<String> {
    let mut live: Vec<&crate::runtime::LocVar> = cl
        .proto
        .locvars
        .iter()
        .filter(|lv| lv.start_pc == 0 && lv.end_pc > 0)
        .collect();
    live.sort_by_key(|lv| lv.reg);
    let lv = live.get(usize::try_from(n.checked_sub(1)?).ok()?)?;
    Some(lv.name.to_string())
}

fn d_setlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (co, arg) = getthread(vm, a);
    let (level, n) = if vm.version() <= LuaVersion::Lua52 {
        let level = check_int(vm, a, arg)?;
        let level = stack_level(vm, co, level, arg + 1)?;
        check_any(vm, a, arg + 2)?;
        (level, check_int(vm, a, arg + 1)?)
    } else {
        let level = check_int(vm, a, arg)?;
        let n = check_int(vm, a, arg + 1)?;
        let level = stack_level(vm, co, level, arg + 1)?;
        check_any(vm, a, arg + 2)?;
        (level, n)
    };
    let val = a.get(vm, arg + 2);
    let name = match find_local(vm, co, level, n) {
        Some((name, LocalSlot::Stack(slot))) => {
            vm.write_thread_slot(co, slot, val);
            Some(name)
        }
        // the value lives in a slot of PUC's C function that luna's native
        // does not have; nothing to write
        Some((name, LocalSlot::Held(_))) => Some(name),
        None => None,
    };
    let r = match name {
        Some(nm) => Value::Str(vm.heap.intern(nm.as_bytes())),
        None => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[r]))
}

/// 1-based upvalue index → raw `upvals[]` index. 5.1 functions keep their
/// environment outside the upvalues, so luna's `_ENV` cell is skipped there.
fn visible_upvalue_index(vm: &Vm, cl: Gc<LuaClosure>, n: i64) -> Option<usize> {
    if n < 1 {
        return None;
    }
    if vm.version() <= LuaVersion::Lua51 {
        return cl
            .proto
            .upvals
            .iter()
            .enumerate()
            .filter(|(_, u)| &*u.name != "_ENV")
            .nth((n - 1) as usize)
            .map(|(idx, _)| idx);
    }
    ((n as usize) <= cl.upvals().len()).then(|| (n - 1) as usize)
}

/// PUC `aux_upvalue`'s name for upvalue `idx` of a Lua closure: `None` when
/// 5.1 has no name for it (a stripped function carries none).
fn upvalue_name(vm: &Vm, cl: Gc<LuaClosure>, idx: usize) -> Option<String> {
    let name = &cl.proto.upvals[idx].name;
    if !name.is_empty() {
        return Some(name.to_string());
    }
    Some(
        match vm.version() {
            LuaVersion::Lua51 => return None,
            LuaVersion::Lua52 => "",
            LuaVersion::Lua53 => "(*no name)",
            _ => "(no name)",
        }
        .to_string(),
    )
}

/// PUC `auxupvalue`: the checked index and function arguments.
fn upvalue_args(vm: &mut Vm, a: Args) -> Result<(Value, i64), LuaError> {
    let n = check_int(vm, a, 1)?;
    Ok((check_function(vm, a, 0)?, n))
}

fn d_getupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (f, n) = upvalue_args(vm, Args::new(fs, nargs))?;
    let found = match f {
        // 5.1 does not let Lua touch C upvalues
        Value::Native(_) if vm.version() == LuaVersion::Lua51 => None,
        Value::Native(nc) => usize::try_from(n - 1)
            .ok()
            .and_then(|i| nc.upvals.get(i).copied())
            .map(|v| (String::new(), v)),
        Value::Closure(cl) => visible_upvalue_index(vm, cl, n).and_then(|idx| {
            upvalue_name(vm, cl, idx).map(|name| (name, vm.upvalue_value(cl, idx)))
        }),
        _ => unreachable!("checked function"),
    };
    match found {
        Some((name, value)) => {
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm, value]))
        }
        None => Ok(vm.nat_return(fs, &[])),
    }
}

fn d_setupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let value = check_any(vm, a, 2)?;
    let (f, n) = upvalue_args(vm, a)?;
    let name = match f {
        Value::Native(_) if vm.version() == LuaVersion::Lua51 => None,
        Value::Native(nc) => match usize::try_from(n - 1) {
            Ok(i) if i < nc.upvals.len() => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { nc.as_mut() }.upvals[i] = value;
                vm.heap
                    .barrier_back(nc.as_ptr() as *mut crate::runtime::heap::GcHeader);
                Some(String::new())
            }
            _ => None,
        },
        Value::Closure(cl) => match visible_upvalue_index(vm, cl, n) {
            Some(idx) => {
                let name = upvalue_name(vm, cl, idx);
                if name.is_some() {
                    vm.upvalue_set_value(cl, idx, value);
                }
                name
            }
            None => None,
        },
        _ => unreachable!("checked function"),
    };
    match name {
        Some(name) => {
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm]))
        }
        None => Ok(vm.nat_return(fs, &[])),
    }
}

/// PUC `lua_upvalueid` of upvalue `n` of `f`: the address identifying it,
/// `None` when out of range (a luna native with no upvalues is PUC's light C
/// function).
fn upvalue_id(f: Value, n: i64) -> Option<*const ()> {
    let i = usize::try_from(n - 1).ok()?;
    match f {
        Value::Closure(cl) => cl.upvals().get(i).map(|u| u.as_ptr() as *const ()),
        Value::Native(nc) => nc.upvals.get(i).map(|v| v as *const Value as *const ()),
        _ => unreachable!("checked function"),
    }
}

/// PUC `checkupval(L, argf, argnup, pnup)`. 5.2/5.3 reject an index out of
/// range; 5.4+ does only when joining (`pnup` given).
fn check_upval(
    vm: &mut Vm,
    a: Args,
    argf: u32,
    argnup: u32,
    joining: bool,
) -> Result<(Value, i64, Option<*const ()>), LuaError> {
    let n = check_int(vm, a, argnup)?;
    let f = check_function(vm, a, argf)?;
    let id = upvalue_id(f, n);
    if id.is_none() && (joining || vm.version() <= LuaVersion::Lua53) {
        return Err(arg_error(vm, argnup + 1, "invalid upvalue index"));
    }
    Ok((f, n, id))
}

fn d_upvalueid(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (_, _, id) = check_upval(vm, Args::new(fs, nargs), 0, 1, false)?;
    let r = id.map_or(Value::Nil, Value::LightUserdata);
    Ok(vm.nat_return(fs, &[r]))
}

fn d_upvaluejoin(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (f1, n1, _) = check_upval(vm, a, 0, 1, true)?;
    let (f2, n2, _) = check_upval(vm, a, 2, 3, true)?;
    let (Value::Closure(f1), Value::Closure(f2)) = (f1, f2) else {
        let argn = if matches!(f1, Value::Native(_)) { 1 } else { 3 };
        return Err(arg_error(vm, argn, "Lua function expected"));
    };
    let uv = f2.upvals()[(n2 - 1) as usize];
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { f1.as_mut() }.upvals_mut()[(n1 - 1) as usize] = uv;
    // f1's upvalue slice just changed; re-gray it so the collector re-traces
    vm.heap
        .barrier_back(f1.as_ptr() as *mut crate::runtime::heap::GcHeader);
    Ok(0)
}

fn d_getuservalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() <= LuaVersion::Lua53 {
        let r = match a.get(vm, 0) {
            Value::Userdata(u) if !a.is_none(0) => u.user_value,
            _ => Value::Nil,
        };
        return Ok(vm.nat_return(fs, &[r]));
    }
    // 5.4+: user value `n` of a full userdata; luna's userdata carry none,
    // so every index is out of range (PUC pushes nil and no flag)
    opt_integer(vm, a, 1, 1)?;
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

fn d_setuservalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    if v >= LuaVersion::Lua54 {
        opt_integer(vm, a, 2, 1)?;
    }
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
    if v >= LuaVersion::Lua54 {
        // no user value slots: `lua_setiuservalue` fails
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { u.as_mut() }.user_value = value;
    vm.heap
        .barrier_back(u.as_ptr() as *mut crate::runtime::heap::GcHeader);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

fn d_traceback(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (co, arg) = getthread(vm, a);
    let default_level = if co.is_none() || vm.is_current_thread(co) {
        1
    } else {
        0
    };
    if vm.version() == LuaVersion::Lua51 {
        return traceback_51(vm, fs, a, co, arg, default_level);
    }
    let msg = if a.is_none(arg) {
        None
    } else {
        crate::vm::argcheck::to_str_bytes(vm, a.get(vm, arg))
    };
    if msg.is_none() && !a.is_none_or_nil(vm, arg) {
        // a message that is not a string is returned untouched
        let m = a.get(vm, arg);
        return Ok(vm.nat_return(fs, &[m]));
    }
    let level = opt_integer(vm, a, arg + 1, default_level)? as i32 as i64;
    let mut out = match msg {
        Some(mut m) => {
            m.push(b'\n');
            m
        }
        None => Vec::new(),
    };
    out.extend_from_slice(b"stack traceback:");
    out.extend(thread_traceback(vm, co, level));
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

/// The level lines of `co`'s traceback; a coroutine killed by an error
/// shows the stack it died with, as PUC leaves a dead thread's stack.
fn thread_traceback(vm: &mut Vm, co: Option<Gc<Coro>>, level: i64) -> Vec<u8> {
    if let Some(co) = co
        && let Some(lines) = co.error_levels.as_ref()
    {
        return crate::vm::callstack::traceback_from_lines(vm.version(), lines, level);
    }
    vm.traceback_lines(co, level)
}

/// PUC 5.1 `db_errorfb`, which works on the raw argument stack: a numeric
/// last-but-one argument is the level (and is popped from the *top*), a
/// non-string message is returned as is, and every argument still on the
/// stack is concatenated in front of the traceback.
fn traceback_51(
    vm: &mut Vm,
    fs: u32,
    a: Args,
    co: Option<Gc<Coro>>,
    arg: u32,
    default_level: i64,
) -> Result<u32, LuaError> {
    let mut stack: Vec<Value> = (arg..a.n).map(|i| a.get(vm, i)).collect();
    let level = match stack.get(1).and_then(|&v| to_num(vm, v)) {
        Some(n) => {
            stack.pop();
            match n {
                crate::numeric::Num::Int(i) => i,
                crate::numeric::Num::Float(f) => f as i64,
            }
        }
        None => default_level,
    };
    let mut out = Vec::new();
    if let Some(&msg) = stack.first() {
        if crate::vm::argcheck::to_str_bytes(vm, msg).is_none() {
            // `return 1` hands back whatever is on top
            let top = *stack.last().expect("non-empty");
            return Ok(vm.nat_return(fs, &[top]));
        }
        // `lua_concat` works from the top down, so the rightmost bad part is
        // the one reported; the C function has no position to add
        if let Some(bad) = stack
            .iter()
            .rev()
            .find(|&&p| crate::vm::argcheck::to_str_bytes(vm, p).is_none())
        {
            let msg = format!("attempt to concatenate a {} value", bad.type_name());
            return Err(vm.plain_err(&msg));
        }
        for &part in &stack {
            out.extend(crate::vm::argcheck::to_str_bytes(vm, part).expect("checked above"));
        }
        out.push(b'\n');
    }
    out.extend_from_slice(b"stack traceback:");
    out.extend(thread_traceback(vm, co, level));
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn d_sethook(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (co, arg) = getthread(vm, a);
    let state = if a.is_none_or_nil(vm, arg) {
        HookState::default()
    } else {
        let mask = check_string(vm, a, arg + 1)?.as_bytes().to_vec();
        let func = check_function(vm, a, arg)?;
        let count = opt_integer(vm, a, arg + 2, 0)? as i32 as i64;
        HookState {
            func: Some(func),
            rust_func: None,
            call: mask.contains(&b'c'),
            ret: mask.contains(&b'r'),
            line: mask.contains(&b'l'),
            count: count > 0,
            count_base: count,
            count_left: count,
        }
    };
    vm.set_hook(co, state);
    Ok(0)
}

fn d_gethook(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (co, _) = getthread(vm, Args::new(fs, nargs));
    let v = vm.version();
    let state = vm.get_hook(co);
    // `lua_sethook` drops a hook whose mask is empty; 5.1/5.2 still report
    // the function the hook table recorded for the thread
    let armed = state.call || state.ret || state.line || state.count;
    let hook = match state.func {
        Some(h) if armed => h,
        None if armed => Value::Str(vm.heap.intern(b"external hook")),
        _ if v >= LuaVersion::Lua54 => return Ok(vm.nat_return(fs, &[Value::Nil])),
        Some(h) if v <= LuaVersion::Lua52 => h,
        _ => Value::Nil,
    };
    let mut mask = Vec::new();
    if state.call {
        mask.push(b'c');
    }
    if state.ret {
        mask.push(b'r');
    }
    if state.line {
        mask.push(b'l');
    }
    let m = Value::Str(vm.heap.intern(&mask));
    Ok(vm.nat_return(fs, &[hook, m, Value::Int(state.count_base)]))
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
    let f = match vm.load(line, b"=(debug command)") {
        Ok(cl) => cl,
        Err(e) => {
            let id = crate::vm::callstack::syntax_chunk_id(v, b"=(debug command)");
            return Err(e.render(&id));
        }
    };
    let err = match vm.call_protected(Value::Closure(f), &[]) {
        Ok(_) => return Ok(()),
        Err(e) => e.0,
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
            let uv = cl.upvals()[env_idx];
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { uv.as_mut() }.set_closed(Value::Table(env_t));
            vm.barrier_forward_upvalue(uv, Value::Table(env_t));
        }
        Value::Coro(co) => {
            // the running thread's globals are the Vm's; a suspended one
            // keeps its own until resumed
            if vm.is_current_thread(Some(co)) {
                vm.set_globals(env_t);
            } else {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { co.as_mut() }.globals = env_t;
                vm.heap
                    .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
            }
        }
        // luna keeps no environment on natives or userdata; PUC's change
        // would be observable only through `getfenv` of that object
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
        Value::Coro(_) | Value::Native(_) | Value::Userdata(_) => Value::Table(vm.globals()),
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[env]))
}
