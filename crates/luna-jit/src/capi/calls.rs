//! Calls from C: `lua_call(k)`, `lua_pcall(k)`, `lua_cpcall`, yields
//! (`lua_yield(k)`) and `lua_error`. The functions that may raise or yield
//! are reached through their C wrappers (`csrc/shim_wrap.c`), which throw
//! what these report.

use super::ccall::{Cont, KFn, LUNA_RECOVER, PendingYield, Wait, pcall_outcome, push_results};
use super::*;
use luna_core::vm::exec::host_c::HostContSpec;

/// The continuation slot of the C function running on `api`'s thread, as
/// the VM needs it to suspend a call, or `None` with no C function.
fn cont_spec(api: &mut Api) -> Option<HostContSpec> {
    let s = api.st();
    let token = s.calls.len().checked_sub(1)?;
    let c = &s.calls[token];
    Some(HostContSpec {
        func_slot: c.func_slot,
        nresults: c.nresults,
        token: token as u32,
    })
}

/// Record `cont` on the running C function, if one is running; a
/// continuation's context is what 5.2's `lua_getctx` reports from then on.
fn set_cont(api: &mut Api, cont: Option<Cont>) {
    if let Some(c) = api.st().calls.last_mut() {
        if let Some(k) = cont {
            c.k_ctx = k.ctx;
        }
        c.cont = cont;
    }
}

/// PUC `lua_callk`: call the function below the top `nargs` values,
/// leaving `nresults` results (-1: all). With `k` and a thread that can
/// yield, a yield inside the call leaves the C function; the resume runs
/// `k` once the call returns.
pub(super) fn callk(api: &mut Api, nargs: c_int, nresults: c_int, k: Option<KFn>, ctx: isize) {
    let n = usize::try_from(nargs).unwrap_or(0);
    let args = api.pop_n(n);
    let f = api.pop();
    let spec = match k {
        Some(k) => {
            set_cont(
                api,
                Some(Cont {
                    k: Some(k),
                    ctx,
                    wait: Wait::Call { nresults },
                }),
            );
            cont_spec(api)
        }
        None => None,
    };
    let hooked = super::hooks::calling_from_hook(api);
    api.vm.host_mark_hook_call(hooked);
    let r = api.vm.host_call(f, &args, spec);
    api.vm.host_mark_hook_call(false);
    match r {
        Ok(vals) => {
            if spec.is_some() {
                set_cont(api, None);
            }
            push_results(api.thread(), vals, nresults);
            let co = api.thread();
            api.vm.heap.barrier_back(co);
        }
        Err(_) if api.vm.host_yielding() => api.g().raised = LUA_YIELD,
        Err(e) => {
            if spec.is_some() {
                set_cont(api, None);
            }
            api.raise(e);
        }
    }
}

/// PUC `lua_pcallk`: `lua_callk` in protected mode, with the message
/// handler at stack index `msgh` (0: none). Returns the status; an error
/// leaves its object where the function was.
pub(super) fn pcallk(
    api: &mut Api,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
    k: Option<KFn>,
    ctx: isize,
) -> c_int {
    let n = usize::try_from(nargs).unwrap_or(0);
    if api.top() - api.base() < n + 1 {
        let s = api.str(b"not enough values on stack");
        api.push(s);
        return LUA_ERRRUN;
    }
    // the handler's index is resolved before the function and arguments
    // are popped
    let handler = (msgh != 0).then(|| api.get_or_nil(msgh));
    let args = api.pop_n(n);
    let f = api.pop();
    let at = api.top();
    let mut pargs = Vec::with_capacity(n + 2);
    pargs.push(f);
    pargs.extend(handler);
    pargs.extend(args);
    let pf = api.vm.host_protected_fn(handler.is_some());
    let errerr_before = api.vm.host_errerr_count();
    let spec = match k {
        Some(k) => {
            let wait = Wait::PCall {
                nresults,
                at,
                errerr_before,
            };
            set_cont(
                api,
                Some(Cont {
                    k: Some(k),
                    ctx,
                    wait,
                }),
            );
            cont_spec(api)
        }
        None => None,
    };
    let hooked = super::hooks::calling_from_hook(api);
    api.vm.host_mark_hook_call(hooked);
    let r = api.vm.host_call(pf, &pargs, spec);
    api.vm.host_mark_hook_call(false);
    let co = api.thread();
    let status = match r {
        Ok(vals) => {
            let outcome = pcall_outcome(api.vm, co, vals, nresults, at, errerr_before);
            match (outcome, k, spec) {
                // in a coroutine the error leaves the C function and its
                // continuation runs with it, as PUC's `lua_pcallk` recovers
                // at the resume
                (Some(status), Some(k), Some(_)) => {
                    set_cont(
                        api,
                        Some(Cont {
                            k: Some(k),
                            ctx,
                            wait: Wait::Recover { status },
                        }),
                    );
                    api.vm.heap.barrier_back(co);
                    api.g().raised = LUNA_RECOVER;
                    return LUA_OK;
                }
                _ => {
                    if spec.is_some() {
                        set_cont(api, None);
                    }
                    outcome.unwrap_or(LUA_OK)
                }
            }
        }
        Err(_) if api.vm.host_yielding() => {
            api.g().raised = LUA_YIELD;
            return LUA_OK;
        }
        // a coroutine closing itself goes on unwinding
        Err(e) if api.vm.host_terminating() => {
            api.raise(e);
            return LUA_OK;
        }
        // an error the protected call could not catch (the C stack was
        // already full): it is still the call's error
        Err(e) => {
            if spec.is_some() {
                set_cont(api, None);
            }
            api.truncate(at);
            api.push(e.0);
            LUA_ERRRUN
        }
    };
    api.vm.heap.barrier_back(co);
    status
}

/// PUC `lua_yieldk`: the running C function yields the top `nresults`
/// values; it leaves at once (5.2+), and the resume runs `k`, or returns
/// the resume's values from it without `k`. 5.1's `lua_yield` returns
/// instead, and the C function must return its result.
pub(super) fn yieldk(api: &mut Api, nresults: c_int, k: Option<KFn>, ctx: isize) -> bool {
    if let Some(msg) = api.vm.host_yield_refusal() {
        api.raise_msg(msg);
        return false;
    }
    if super::hooks::hook_yield(api) {
        return false;
    }
    api.st().pending_yield = Some(PendingYield {
        n: nresults,
        k,
        ctx,
    });
    true
}

/// PUC `lua_callk`.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_callk`; the C
// wrapper `luna_c_lua_callk` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_callk(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    ctx: isize,
    k: Option<LuaKFunction>,
) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    callk(&mut api, nargs, nresults, k.map(KFn::K53), ctx);
}

/// 5.2 `lua_callk`, whose context is an `int` and whose continuation is a
/// `lua_CFunction`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_luna_callk_52`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_callk_52(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    ctx: c_int,
    k: Option<LuaCFunction>,
) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    callk(&mut api, nargs, nresults, k.map(KFn::K52), ctx as isize);
}

/// 5.1 `lua_call`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_call`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_call(L: *mut LuaState, nargs: c_int, nresults: c_int) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    callk(&mut api, nargs, nresults, None, 0);
}

/// PUC `lua_pcallk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_pcallk`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pcallk(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
    ctx: isize,
    k: Option<LuaKFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    pcallk(&mut api, nargs, nresults, msgh, k.map(KFn::K53), ctx)
}

/// 5.2 `lua_pcallk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_luna_pcallk_52`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_pcallk_52(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
    ctx: c_int,
    k: Option<LuaCFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    pcallk(
        &mut api,
        nargs,
        nresults,
        msgh,
        k.map(KFn::K52),
        ctx as isize,
    )
}

/// Call the function below the top `nargs` values in protected mode (PUC
/// `lua_pcall`; a function in 5.1, a macro over `lua_pcallk` after). It
/// cannot yield, so it never throws and needs no C wrapper.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pcall`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pcall(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    pcallk(&mut api, nargs, nresults, msgh, None, 0)
}

/// PUC 5.1 `lua_cpcall`: call `func` with a light userdata of `ud` as its
/// only argument, in protected mode; the stack is left as it was, plus the
/// error object on an error.
///
/// # Safety
/// As [`lua_pcall`]; `func` is a C function.
// SAFETY: no other item in the link is named `lua_cpcall`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_cpcall(
    L: *mut LuaState,
    func: LuaCFunction,
    ud: *mut c_void,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    let f = new_c_closure(&mut api, func, 0);
    api.push(f);
    api.push(Value::LightUserdata(ud.cast_const().cast()));
    pcallk(&mut api, 1, 0, 0, None, 0)
}

/// PUC `lua_yieldk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_yieldk`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_yieldk(
    L: *mut LuaState,
    nresults: c_int,
    ctx: isize,
    k: Option<LuaKFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    if yieldk(&mut api, nresults, k.map(KFn::K53), ctx) {
        api.g().raised = LUA_YIELD;
    }
    0
}

/// 5.2 `lua_yieldk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_luna_yieldk_52`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_yieldk_52(
    L: *mut LuaState,
    nresults: c_int,
    ctx: c_int,
    k: Option<LuaCFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    if yieldk(&mut api, nresults, k.map(KFn::K52), ctx as isize) {
        api.g().raised = LUA_YIELD;
    }
    0
}

/// PUC 5.1 `lua_yield`: record the yield and return; the C function must
/// return the result (`return lua_yield(L, n);`).
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_yield`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_yield(L: *mut LuaState, nresults: c_int) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    yieldk(&mut api, nresults, None, 0);
    -1
}

/// 5.2 `lua_getctx`: the status a continuation was called with
/// (`LUA_YIELD`, or the error of a protected call), and its context;
/// `LUA_OK` in the C function itself.
///
/// # Safety
/// `L` is a live thread of an open state; `ctx` is null or writable.
// SAFETY: no other item in the link is named `lua_getctx`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getctx(L: *mut LuaState, ctx: *mut c_int) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    let Some(c) = api.st().calls.last() else {
        return LUA_OK;
    };
    if c.k_status == LUA_OK {
        return LUA_OK;
    }
    let (status, k) = (c.k_status, c.k_ctx);
    if !ctx.is_null() {
        // SAFETY: `ctx` is non-null and writable (# Safety)
        unsafe { *ctx = k as c_int };
    }
    status
}

/// `lua_error`'s Rust side: the error object is on top of `L`.
///
/// # Safety
/// `L` is a live thread of an open state.
// SAFETY: no other item in the link is named `luna_capi_error_prepare`; the
// C function `lua_error` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_error_prepare(L: *mut LuaState) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    if api.top() == api.base() {
        api.push(Value::Nil);
    }
    api.g().err_from = L;
}

/// A C closure of `f` with `n` upvalues popped from the stack (PUC
/// `lua_pushcclosure`), not yet pushed. In 5.1 it takes the environment
/// of the running C function, or the thread's globals.
pub(super) fn new_c_closure(api: &mut Api, f: LuaCFunction, n: usize) -> Value {
    let ups = api.pop_n(n);
    let env = if api.version() == LuaVersion::Lua51 {
        match api.running_c().map(|nc| nc.upvals[1]) {
            Some(Value::Table(t)) => Value::Table(t),
            _ => Value::Table(api.thread_globals()),
        }
    } else {
        Value::Nil
    };
    let mut upvals = Vec::with_capacity(api::C_UPVALS + n);
    upvals.push(Value::LightUserdata(f as *const ()));
    upvals.push(env);
    upvals.extend(ups);
    let trampoline: luna_core::runtime::value::NativeFn = ccall::capi_trampoline;
    // from 5.2 on a C function without upvalues is a light C function,
    // equal to every other push of the same pointer
    if n == 0 && api.version() >= LuaVersion::Lua52 {
        return api.vm.host_light_fn(f as usize, |vm| {
            vm.native_with(trampoline, upvals.into_boxed_slice())
        });
    }
    api.vm.native_with(trampoline, upvals.into_boxed_slice())
}
