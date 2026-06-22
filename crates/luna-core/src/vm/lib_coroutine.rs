//! coroutine library (P05): create / resume / yield / wrap / status / running /
//! isyieldable / close. The heavy lifting (context swapping, the yield signal)
//! lives on `Vm` in exec.rs; these are the thin library wrappers.

use crate::runtime::{CoroStatus, Value};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_coroutine(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f: crate::runtime::value::NativeFn| {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        let fv = vm.native(f);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut vm.heap, k, fv).expect("valid key");
    };
    set(vm, "create", co_create);
    set(vm, "resume", co_resume);
    set(vm, "yield", co_yield);
    set(vm, "status", co_status);
    set(vm, "running", co_running);
    set(vm, "isyieldable", co_isyieldable);
    set(vm, "wrap", co_wrap);
    set(vm, "close", co_close);
    vm.set_global("coroutine", Value::Table(t));
    vm.barrier_back_table(t);
}

/// Collect a native call's `nargs` arguments into an owned vector.
fn collect_args(vm: &Vm, fs: u32, nargs: u32) -> Vec<Value> {
    (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect()
}

fn co_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let body = vm.nat_arg(fs, nargs, 0);
    if !matches!(body, Value::Closure(_) | Value::Native(_)) {
        return Err(arg_error(vm, 1, "create", "function expected"));
    }
    let co = vm.new_coro(body);
    Ok(vm.nat_return(fs, &[Value::Coro(co)]))
}

fn co_resume(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Coro(co) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "resume", "coroutine expected"));
    };
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
                let msg = vm
                    .heap
                    .intern(b"too many results to resume");
                return Ok(vm.nat_return(fs, &[Value::Bool(false), Value::Str(msg)]));
            }
            let mut out = Vec::with_capacity(vals.len() + 1);
            out.push(Value::Bool(true));
            out.append(&mut vals);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => Ok(vm.nat_return(fs, &[Value::Bool(false), e.0])),
    }
}

fn co_yield(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if let Some(msg) = vm.yield_barrier() {
        return Err(raise_str(vm, msg));
    }
    let vals = collect_args(vm, fs, nargs);
    Err(vm.do_yield(fs, vals))
}

fn co_status(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Coro(co) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "status", "coroutine expected"));
    };
    let s = vm.coro_status_str(co);
    let v = Value::Str(vm.heap.intern(s.as_bytes()));
    Ok(vm.nat_return(fs, &[v]))
}

fn co_running(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let (thread, is_main) = vm.running_thread();
    // PUC 5.1 `coroutine.running()` returned nil for the main thread; 5.2+
    // changed it to return the main thread handle plus an `is_main` flag.
    // closure.lua 5.1's `assert(coroutine.running() == nil)` baseline relies
    // on the older convention.
    if vm.version() <= crate::version::LuaVersion::Lua51 && is_main {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    Ok(vm.nat_return(fs, &[thread, Value::Bool(is_main)]))
}

fn co_isyieldable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let co = match vm.nat_arg(fs, nargs, 0) {
        Value::Coro(c) => Some(c),
        _ => None,
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
    let args = collect_args(vm, fs, nargs);
    match vm.resume_coro(co, args) {
        Ok(vals) => Ok(vm.nat_return(fs, &vals)),
        Err(e) => Err(e),
    }
}

fn co_wrap(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let body = vm.nat_arg(fs, nargs, 0);
    if !matches!(body, Value::Closure(_) | Value::Native(_)) {
        return Err(arg_error(vm, 1, "wrap", "function expected"));
    }
    let co = vm.new_coro(body);
    let f = vm.native_with(co_wrapped, Box::new([Value::Coro(co)]));
    Ok(vm.nat_return(fs, &[f]))
}

fn co_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC 5.5: the coroutine argument is optional, defaulting to the running
    // thread (closing itself).
    let co = match vm.nat_arg(fs, nargs, 0) {
        Value::Coro(c) => c,
        Value::Nil if nargs == 0 => match vm.current_coro() {
            Some(c) => c,
            None => return Err(raise_str(vm, "cannot close main thread")),
        },
        _ => return Err(arg_error(vm, 1, "close", "coroutine expected")),
    };
    // PUC 5.4 `auxstatus` reports a coroutine as "running" when it is the
    // currently-executing thread — that path errors with "cannot close a
    // running coroutine". 5.5 instead lets the re-entrant call succeed (the
    // outer close finishes the work). The condition is the same as luna's
    // close_coro re-entrant guard.
    if vm.version() < crate::version::LuaVersion::Lua55
        && vm.current_coro().is_some_and(|c| c.ptr_eq(co))
    {
        return Err(raise_str(vm, "cannot close a running coroutine"));
    }
    match vm.effective_coro_status(co) {
        CoroStatus::Dead | CoroStatus::Suspended => match vm.close_coro(co) {
            // died with an error, or a __close handler raised: report (false, e)
            Ok(Some(e)) => Ok(vm.nat_return(fs, &[Value::Bool(false), e])),
            Ok(None) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
            Err(e) => Ok(vm.nat_return(fs, &[Value::Bool(false), e.0])),
        },
        CoroStatus::Normal => Err(raise_str(vm, "cannot close a normal coroutine")),
        CoroStatus::Running => {
            // PUC 5.5 made `coroutine.close` on the main thread its own
            // distinct error ("cannot close 'main' coroutine") and lets a
            // running thread close *itself* by running its to-be-closed
            // handlers in place. Earlier dialects roll both cases up into
            // "cannot close a running coroutine" — 5.4 coroutine.lua :150
            // matches `string.find(msg, "running")`.
            if vm.version() >= crate::version::LuaVersion::Lua55 {
                if vm.is_main_coro(co) {
                    return Err(raise_str(vm, "cannot close 'main' coroutine"));
                }
                if vm.current_coro().is_some_and(|c| c.ptr_eq(co)) {
                    return Err(vm.close_running());
                }
            }
            Err(raise_str(vm, "cannot close a running coroutine"))
        }
    }
}
