//! Calls from C: `lua_call(k)`, `lua_pcall(k)`, `lua_cpcall`, yields
//! (`lua_yield(k)`) and `lua_error`. The functions that may raise or yield
//! are reached through their C wrappers (`csrc/shim_wrap.c`), which throw
//! what these report.

use super::ccall::{
    Cont, KFn, LUNA_RECOVER, PendingYield, Wait, pcall_failed, pcall_outcome, push_results,
};
use super::*;
use luna_core::vm::exec::host_c::HostContSpec;

mod entries;
pub use entries::*;

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

/// The function below the top `n` values, its C stack index, and the
/// values. They stay on the stack while the call runs, as PUC keeps them in
/// the callee's frame, so they stay rooted; the results replace them.
fn call_operands(api: &mut Api, n: usize) -> (usize, Value, Vec<Value>) {
    let base = api.base();
    let top = api.top();
    let from = top.saturating_sub(n).max(base);
    let args = api.stack()[from..top].to_vec();
    if from > base {
        (from - 1, api.stack()[from - 1], args)
    } else {
        (from, Value::Nil, args)
    }
}

/// The continuation `k` of the running C function set for a call that may
/// yield; what the call is told of it.
fn with_cont(api: &mut Api, k: KFn, ctx: isize, wait: Wait) -> Option<HostContSpec> {
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

/// Call `f` with `args` in the place of its operands at `at`, as PUC's
/// runs where they are (a tail call there leaves nothing of `f`).
fn run_call(
    api: &mut Api,
    at: usize,
    f: Value,
    args: &[Value],
    spec: Option<HostContSpec>,
) -> Result<Vec<Value>, LuaError> {
    api.truncate(at);
    let (hooked, below) = (super::hooks::calling_from_hook(api), at - api.base());
    api.vm.host_mark_hook_call(hooked);
    let r = api.vm.host_call(f, args, spec, Some(below as u32));
    api.vm.host_mark_hook_call(false);
    r
}

/// PUC `lua_callk`: call the function below the top `nargs` values,
/// leaving `nresults` results (-1: all). With `k` and a thread that can
/// yield, a yield inside the call leaves the C function; the resume runs
/// `k` once the call returns.
pub(super) fn callk(api: &mut Api, nargs: c_int, nresults: c_int, k: Option<KFn>, ctx: isize) {
    let n = usize::try_from(nargs).unwrap_or(0);
    let (at, f, args) = call_operands(api, n);
    let spec = k.and_then(|k| with_cont(api, k, ctx, Wait::Call { nresults, at }));
    let r = run_call(api, at, f, &args, spec);
    match r {
        Ok(vals) => {
            if spec.is_some() {
                set_cont(api, None);
            }
            let co = api.thread();
            if let Err(e) = push_results(api.vm, co, at, vals, nresults) {
                api.raise(e);
            }
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
        return api.push_status(b"not enough values on stack", LUA_ERRRUN);
    }
    let handler = (msgh != 0).then(|| api.get_or_nil(msgh));
    let (at, f, args) = call_operands(api, n);
    let mut pargs = Vec::with_capacity(n + 2);
    pargs.push(f);
    pargs.extend(handler);
    pargs.extend(args);
    // the operands are still on the stack, kept by an emergency collection
    let pf = match api.vm.host_protected_fn(handler.is_some()) {
        Ok(pf) => pf,
        Err(e) => {
            api.truncate(at);
            api.push(e.0);
            return LUA_ERRMEM;
        }
    };
    let errerr_before = api.vm.special_errors();
    let wait = Wait::PCall {
        nresults,
        at,
        errerr_before,
    };
    let spec = k.and_then(|k| with_cont(api, k, ctx, wait));
    let r = run_call(api, at, pf, &pargs, spec);
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
                    // SAFETY: `co` is a live thread of this Vm; the heap is written in place
                    unsafe { api.vm.heap_mut().barrier_back(co) };
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
        Err(e) => {
            if spec.is_some() {
                set_cont(api, None);
            }
            pcall_failed(api.vm, co, at, e, errerr_before)
        }
    };
    // SAFETY: `co` is a live thread of this Vm; the heap is written in place
    unsafe { api.vm.heap_mut().barrier_back(co) };
    status
}

/// PUC `lua_yieldk`: the running C function yields the top `nresults`
/// values; it leaves at once (5.2+), and the resume runs `k`, or returns
/// the resume's values from it without `k`. 5.1's `lua_yield` returns
/// instead, and the C function must return its result.
pub(super) fn yieldk(api: &mut Api, nresults: c_int, k: Option<KFn>, ctx: isize) -> bool {
    if let Some(msg) = api.vm.host_yield_refusal() {
        // inside a hook the interrupted Lua function is where it happens
        if super::hooks::in_lua_hook(api) {
            let e = api.vm.runtime_error(msg);
            api.raise(e);
        } else {
            api.raise_msg(msg);
        }
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
