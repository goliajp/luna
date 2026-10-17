//! coroutine library: create / resume / yield / wrap / status / running /
//! isyieldable / close. The heavy lifting (context swapping, the yield signal)
//! lives on `Vm` in exec.rs; these are the thin library wrappers, shaped per
//! dialect after 5.1's lbaselib and 5.2–5.5's lcorolib.

use crate::runtime::{Coro, CoroStatus, Gc, Table, TableError, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_function, type_error};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod close;
mod wrap;
pub(crate) use close::*;
use wrap::co_wrap;

/// Open the `coroutine` library, failing when memory runs out.
pub(crate) fn try_open_coroutine(vm: &mut Vm) -> Result<(), LuaError> {
    let t = vm.lib_table(8)?;
    let set =
        |vm: &mut Vm, name: &str, f: crate::runtime::value::NativeFn| -> Result<(), LuaError> {
            let k = vm.str_value(name.as_bytes())?;
            let fv = vm.lib_native(f)?;
            vm.lib_set_value(t, k, fv)
        };
    set(vm, "create", co_create)?;
    set(vm, "yield", co_yield)?;
    set(vm, "status", co_status)?;
    set(vm, "running", co_running)?;
    // the threads now inside a wrapped call, which `resume_refusal` needs:
    // luna's own table, which PUC has none of
    vm.heap.puc.silent += 1;
    let w = vm.alloc_table();
    vm.heap.puc.silent -= 1;
    let w = w?;
    vm.heap.own_table(w);
    let in_wrap = Value::Table(w);
    for (name, f) in [
        ("resume", co_resume as crate::runtime::value::NativeFn),
        ("wrap", co_wrap),
    ] {
        let k = vm.str_value(name.as_bytes())?;
        let fv = vm.lib_native_with(f, &[in_wrap])?;
        vm.lib_set_value(t, k, fv)?;
    }
    if vm.version() >= LuaVersion::Lua53 {
        set(vm, "isyieldable", co_isyieldable)?;
    }
    if vm.version() >= LuaVersion::Lua54 {
        set(vm, "close", co_close)?;
    }
    vm.lib_set_global("coroutine", Value::Table(t))?;
    vm.barrier_back_table(t);
    Ok(())
}

/// Collect a native call's `nargs` arguments into an owned vector.
fn collect_args(vm: &Vm, fs: u32, nargs: u32) -> Vec<Value> {
    (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect()
}

/// The body check of `create`/`wrap`: 5.1 insists on a Lua function
/// (`luaL_argcheck`, a C function cannot be a coroutine body there); 5.2+ is
/// `luaL_checktype(L, 1, LUA_TFUNCTION)`.
fn check_body(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    let v = a.get(vm, 0);
    if vm.version() <= LuaVersion::Lua51 {
        return match v {
            Value::Closure(_) if !a.is_none(0) => Ok(v),
            _ => Err(arg_error(vm, 1, "Lua function expected")),
        };
    }
    check_function(vm, a, 0)
}

fn co_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let co = new_body_coro(vm, Args::new(fs, nargs))?;
    vm.nat_ret(fs, &[Value::Coro(co)])
}

/// The coroutine of `create`/`wrap` over argument 1: 5.1 makes the thread
/// before it checks the argument (`luaB_cocreate`), later dialects after.
fn new_body_coro(vm: &mut Vm, a: Args) -> Result<Gc<Coro>, LuaError> {
    if vm.version() > LuaVersion::Lua51 {
        let body = check_body(vm, a)?;
        return vm.try_new_coro(body);
    }
    // made over the argument whatever it is: a refused one leaves garbage
    let co = vm.try_new_coro(a.get(vm, 0))?;
    // the new thread is pushed over the arguments
    vm.native_push(1);
    vm.lib_roots.push(Value::Coro(co));
    let body = check_body(vm, a);
    vm.lib_roots.pop();
    vm.native_pop(1);
    body.map(|_| co)
}

/// The thread argument (lcorolib `getco`): an argcheck without the offending
/// type on ≤5.3 ("coroutine expected" before 5.3 renamed it), a full
/// `luaL_argexpected` type error from 5.4 on.
fn check_co(vm: &mut Vm, a: Args) -> Result<Gc<Coro>, LuaError> {
    match a.get(vm, 0) {
        Value::Coro(co) if !a.is_none(0) => Ok(co),
        _ => Err(match vm.version() {
            LuaVersion::Lua51 | LuaVersion::Lua52 => arg_error(vm, 1, "coroutine expected"),
            LuaVersion::Lua53 => arg_error(vm, 1, "thread expected"),
            _ => type_error(vm, a, 0, "thread"),
        }),
    }
}

/// Why `co` cannot be resumed, as the dialect words it, or `None` if it is
/// suspended. 5.1's `auxresume` names every status. 5.2/5.3 first report a
/// thread with an empty frame as dead (`lua_gettop(co) == 0`) and leave the
/// rest to `lua_resume`'s "non-suspended". Two threads have an empty frame:
/// the running one inside a wrapped call of its own made without arguments
/// (`own_frame_empty`), and one waiting in a wrapped call it made, whose
/// arguments went to the coroutine it resumed (marked in `in_wrap`).
fn resume_refusal(
    vm: &Vm,
    co: Gc<Coro>,
    own_frame_empty: bool,
    in_wrap: Gc<Table>,
) -> Option<String> {
    let status = vm.effective_coro_status(co);
    if status == CoroStatus::Suspended {
        return None;
    }
    let empty = if vm.current_coro().is_some_and(|c| c.ptr_eq(co)) {
        own_frame_empty
    } else {
        in_wrap.get(Value::Coro(co)).truthy()
    };
    Some(match vm.version() {
        LuaVersion::Lua51 => format!("cannot resume {} coroutine", vm.coro_status_str(co)),
        LuaVersion::Lua52 | LuaVersion::Lua53 if status == CoroStatus::Dead || empty => {
            "cannot resume dead coroutine".to_string()
        }
        _ if status == CoroStatus::Dead => "cannot resume dead coroutine".to_string(),
        _ => "cannot resume non-suspended coroutine".to_string(),
    })
}

fn upval_table(vm: &Vm, fs: u32, i: usize) -> Gc<Table> {
    match vm.nat_upval(fs, i) {
        Value::Table(t) => t,
        _ => unreachable!("coroutine natives keep their state table here"),
    }
}

/// Mark or unmark the running thread as waiting in a wrapped call (only
/// 5.2/5.3 read the mark).
fn mark_in_wrap(vm: &mut Vm, in_wrap: Gc<Table>, on: bool) -> Result<(), LuaError> {
    if !matches!(vm.version(), LuaVersion::Lua52 | LuaVersion::Lua53) {
        return Ok(());
    }
    let me = vm.running_thread().0;
    let v = if on { Value::Bool(true) } else { Value::Nil };
    // the table is reachable through `debug.getupvalue`, so a script can
    // fill it; only adding a key can fail
    // SAFETY: `in_wrap` is the table the running wrap/resume native carries as its upvalue, so the native keeps it alive; no reference into it is live across the `set`, which does not collect
    match vm.table_op(|vm| unsafe { in_wrap.as_mut() }.set(&mut vm.heap, me, v)) {
        Ok(()) => {}
        Err(e @ (TableError::ReadOnly | TableError::Memory(_))) => return Err(vm.table_error(e)),
        Err(_) => return Err(vm.rt_err("table overflow")),
    }
    vm.barrier_back_table(in_wrap);
    Ok(())
}

/// The string of a refusal [`resume_refusal`] worded: 5.1's `auxresume`
/// formats it (`lua_pushfstring`), later dialects push a literal.
fn refusal_value(vm: &mut Vm, msg: &str) -> Result<Value, LuaError> {
    let status = msg
        .strip_prefix("cannot resume ")
        .and_then(|m| m.strip_suffix(" coroutine"));
    match status {
        Some(st) if vm.version() == LuaVersion::Lua51 => {
            let args = [crate::runtime::puc_fmt::FmtArg::S(st.as_bytes())];
            let s = vm.pushfstring(b"cannot resume %s coroutine", &args)?;
            Ok(Value::Str(s))
        }
        _ => vm.str_value(msg.as_bytes()),
    }
}

fn co_resume(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let co = check_co(vm, Args::new(fs, nargs))?;
    let in_wrap = upval_table(vm, fs, 0);
    if let Some(msg) = resume_refusal(vm, co, false, in_wrap) {
        let m = refusal_value(vm, &msg)?;
        return vm.nat_ret(fs, &[Value::Bool(false), m]);
    }
    // 5.1 raises when `lua_checkstack` on either thread refuses
    // (`LUAI_MAXCSTACK` slots above the thread's base, which holds the
    // coroutine's body on one side and the coroutine on the other)
    let v51 = vm.version() == LuaVersion::Lua51;
    let max51 = crate::vm::argcheck::MAXCSTACK_51 as u32;
    if v51 && nargs > max51 {
        return Err(raise_str(vm, "too many arguments to resume"));
    }
    let args: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    match vm.resume_coro(co, args) {
        Ok(mut vals) => {
            if v51 && vals.len() as u32 >= max51 {
                // the arguments moved to the coroutine
                vm.native_settop(1);
                return Err(raise_str(vm, "too many results to resume"));
            }
            // PUC `auxresume` gates the transfer on `lua_checkstack(L,
            // nres + 1)` on the resuming thread, whose top is past the
            // coroutine argument once the other arguments moved to it
            // (coroutine.lua :530 resumes ones that return `lim - 10` …
            // `lim + 1` values)
            if !vm.checkstack(fs + 2, vals.len() as i64 + 1) {
                let msg = vm.alloc_str(b"too many results to resume")?;
                return vm.nat_ret(fs, &[Value::Bool(false), Value::Str(msg)]);
            }
            let mut out = Vec::with_capacity(vals.len() + 1);
            out.push(Value::Bool(true));
            out.append(&mut vals);
            vm.nat_ret(fs, &out)
        }
        Err(e) => {
            let e = death_value(vm, e.0);
            vm.nat_ret(fs, &[Value::Bool(false), e])
        }
    }
}

/// A coroutine's error object as its resumer sees it. A coroutine thread has
/// no message handler, so 5.5's `luaG_errormsg` always swaps a nil error for
/// its placeholder text before the error leaves the thread.
fn death_value(vm: &mut Vm, e: Value) -> Value {
    if e.is_nil() && vm.version() >= LuaVersion::Lua55 {
        return vm.str_value(b"<no error object>").unwrap_or_else(|e| e.0);
    }
    e
}

fn co_yield(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // `lua_yield` raises through `luaG_runerror` with the yield native itself
    // as the running call, so the message carries no position.
    if let Some(msg) = vm.yield_barrier() {
        return Err(vm.plain_err(msg));
    }
    let vals = collect_args(vm, fs, nargs);
    Err(vm.do_yield(fs, vals))
}

fn co_status(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let co = check_co(vm, Args::new(fs, nargs))?;
    let s = vm.coro_status_str(co);
    let v = vm.str_value(s.as_bytes())?;
    vm.nat_ret(fs, &[v])
}

fn co_running(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let (thread, is_main) = vm.running_thread();
    // 5.1 returns a single value: the running coroutine, or nil on the main
    // thread (which was not a coroutine there). 5.2 added the main thread's
    // handle and the is-main flag.
    if vm.version() <= LuaVersion::Lua51 {
        let v = if is_main { Value::Nil } else { thread };
        return vm.nat_ret(fs, &[v]);
    }
    vm.nat_ret(fs, &[thread, Value::Bool(is_main)])
}

fn co_isyieldable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.3 asks about the running thread only; 5.4 added the optional thread.
    let a = Args::new(fs, nargs);
    let co = if vm.version() >= LuaVersion::Lua54 && !a.is_none(0) {
        Some(check_co(vm, a)?)
    } else {
        None
    };
    let y = vm.is_yieldable(co);
    vm.nat_ret(fs, &[Value::Bool(y)])
}
