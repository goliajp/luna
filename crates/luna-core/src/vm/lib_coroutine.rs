//! coroutine library (P05): create / resume / yield / wrap / status / running /
//! isyieldable / close. The heavy lifting (context swapping, the yield signal)
//! lives on `Vm` in exec.rs; these are the thin library wrappers, shaped per
//! dialect after 5.1's lbaselib and 5.2–5.5's lcorolib.

use crate::runtime::{Coro, CoroStatus, Gc, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_function, type_error};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_coroutine(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f: crate::runtime::value::NativeFn| {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        let fv = vm.native(f);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    set(vm, "create", co_create);
    set(vm, "resume", co_resume);
    set(vm, "yield", co_yield);
    set(vm, "status", co_status);
    set(vm, "running", co_running);
    set(vm, "wrap", co_wrap);
    if vm.version() >= LuaVersion::Lua53 {
        set(vm, "isyieldable", co_isyieldable);
    }
    if vm.version() >= LuaVersion::Lua54 {
        set(vm, "close", co_close);
    }
    vm.set_global("coroutine", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
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
    let body = check_body(vm, Args::new(fs, nargs))?;
    let co = vm.new_coro(body);
    Ok(vm.nat_return(fs, &[Value::Coro(co)]))
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
/// thread with an empty frame as dead (`lua_gettop(co) == 0`) — the running
/// thread seen from inside a wrapped call of its own with no arguments is
/// such a thread — and leave the rest to `lua_resume`'s "non-suspended".
fn resume_refusal(vm: &Vm, co: Gc<Coro>, own_frame_empty: bool) -> Option<String> {
    let status = vm.effective_coro_status(co);
    if status == CoroStatus::Suspended {
        return None;
    }
    let running_self = vm.current_coro().is_some_and(|c| c.ptr_eq(co));
    Some(match vm.version() {
        LuaVersion::Lua51 => format!("cannot resume {} coroutine", vm.coro_status_str(co)),
        LuaVersion::Lua52 | LuaVersion::Lua53
            if status == CoroStatus::Dead || (running_self && own_frame_empty) =>
        {
            "cannot resume dead coroutine".to_string()
        }
        _ if status == CoroStatus::Dead => "cannot resume dead coroutine".to_string(),
        _ => "cannot resume non-suspended coroutine".to_string(),
    })
}

fn co_resume(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let co = check_co(vm, Args::new(fs, nargs))?;
    if let Some(msg) = resume_refusal(vm, co, false) {
        let m = Value::Str(vm.heap.intern(msg.as_bytes()));
        return Ok(vm.nat_return(fs, &[Value::Bool(false), m]));
    }
    let args: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    match vm.resume_coro(co, args) {
        Ok(mut vals) => {
            // PUC `auxresume` (lcorolib.c) gates the return-value transfer on
            // `lua_checkstack(L, nres + 1)` *against the parent thread's
            // stack room* — a coroutine that produces a million values into
            // its own stack still cannot deliver them to a caller with no
            // room to receive. coroutine.lua :530's "bug (stack overflow)"
            // series asserts this by spinning up coroutines that build a
            // table of `lim - 10` … `lim + 1` entries and asserts every
            // resume fails.
            if (vals.len() as i64) + 1 > vm.stack_room() {
                let msg = vm.heap.intern(b"too many results to resume");
                return Ok(vm.nat_return(fs, &[Value::Bool(false), Value::Str(msg)]));
            }
            let mut out = Vec::with_capacity(vals.len() + 1);
            out.push(Value::Bool(true));
            out.append(&mut vals);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => {
            let e = death_value(vm, e.0);
            Ok(vm.nat_return(fs, &[Value::Bool(false), e]))
        }
    }
}

/// A coroutine's error object as its resumer sees it. A coroutine thread has
/// no message handler, so 5.5's `luaG_errormsg` always swaps a nil error for
/// its placeholder text before the error leaves the thread.
fn death_value(vm: &mut Vm, e: Value) -> Value {
    if e.is_nil() && vm.version() >= LuaVersion::Lua55 {
        return Value::Str(vm.heap.intern(b"<no error object>"));
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
    let v = Value::Str(vm.heap.intern(s.as_bytes()));
    Ok(vm.nat_return(fs, &[v]))
}

fn co_running(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let (thread, is_main) = vm.running_thread();
    // 5.1 returns a single value: the running coroutine, or nil on the main
    // thread (which was not a coroutine there). 5.2 added the main thread's
    // handle and the is-main flag.
    if vm.version() <= LuaVersion::Lua51 {
        let v = if is_main { Value::Nil } else { thread };
        return Ok(vm.nat_return(fs, &[v]));
    }
    Ok(vm.nat_return(fs, &[thread, Value::Bool(is_main)]))
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
    Ok(vm.nat_return(fs, &[Value::Bool(y)]))
}

/// The function returned by `coroutine.wrap`: upvalue [0] holds the coroutine;
/// resuming it propagates errors instead of returning `(false, err)`.
fn co_wrapped(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Coro(co) = vm.nat_upval(fs, 0) else {
        unreachable!("wrap upvalue is a coroutine");
    };
    let err = match resume_refusal(vm, co, nargs == 0) {
        Some(msg) => Value::Str(vm.heap.intern(msg.as_bytes())),
        None => {
            let args = collect_args(vm, fs, nargs);
            match vm.resume_coro(co, args) {
                Ok(vals) => return Ok(vm.nat_return(fs, &vals)),
                Err(e) => death_value(vm, e.0),
            }
        }
    };
    Err(LuaError(wrap_where(vm, err)))
}

/// `luaB_auxwrap` prefixes a string error with `luaL_where(L, 1)` — the
/// position of whoever called the wrapped function. ≤5.2 test `lua_isstring`,
/// so a number error is converted and prefixed too; 5.3+ only strings.
fn wrap_where(vm: &mut Vm, err: Value) -> Value {
    let text = match err {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(_) | Value::Float(_) if vm.version() <= LuaVersion::Lua52 => {
            crate::vm::argcheck::to_str_bytes(vm, err).expect("a number converts")
        }
        _ => return err,
    };
    let mut out = vm.position_prefix().unwrap_or_default().into_bytes();
    out.extend_from_slice(&text);
    Value::Str(vm.heap.intern(&out))
}

fn co_wrap(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let body = check_body(vm, Args::new(fs, nargs))?;
    let co = vm.new_coro(body);
    let f = vm.native_with(co_wrapped, Box::new([Value::Coro(co)]));
    Ok(vm.nat_return(fs, &[f]))
}

fn co_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // 5.5 `getoptco`: with no argument, close the running thread itself.
    let co = if vm.version() >= LuaVersion::Lua55 && a.is_none(0) {
        match vm.current_coro() {
            Some(c) => c,
            None => return Err(raise_str(vm, "cannot close main thread")),
        }
    } else {
        check_co(vm, a)?
    };
    // PUC 5.4 `auxstatus` reports a coroutine as "running" when it is the
    // currently-executing thread — that path errors with "cannot close a
    // running coroutine". 5.5 instead lets the re-entrant call succeed (the
    // outer close finishes the work). The condition is the same as luna's
    // close_coro re-entrant guard.
    if vm.version() < LuaVersion::Lua55 && vm.current_coro().is_some_and(|c| c.ptr_eq(co)) {
        return Err(raise_str(vm, "cannot close a running coroutine"));
    }
    match vm.effective_coro_status(co) {
        CoroStatus::Dead | CoroStatus::Suspended => match vm.close_coro(co) {
            // died with an error, or a __close handler raised: report (false, e)
            Ok(Some(e)) => {
                let e = death_value(vm, e);
                Ok(vm.nat_return(fs, &[Value::Bool(false), e]))
            }
            Ok(None) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
            Err(e) => {
                let e = death_value(vm, e.0);
                Ok(vm.nat_return(fs, &[Value::Bool(false), e]))
            }
        },
        CoroStatus::Normal => Err(raise_str(vm, "cannot close a normal coroutine")),
        CoroStatus::Running => {
            // 5.5 refuses the main thread by name and lets a running thread
            // close *itself* by running its to-be-closed handlers in place;
            // 5.4 rolls both into "cannot close a running coroutine".
            if vm.version() >= LuaVersion::Lua55 {
                if vm.is_main_coro(co) {
                    return Err(raise_str(vm, "cannot close main thread"));
                }
                if vm.current_coro().is_some_and(|c| c.ptr_eq(co)) {
                    return Err(vm.close_running());
                }
            }
            Err(raise_str(vm, "cannot close a running coroutine"))
        }
    }
}
