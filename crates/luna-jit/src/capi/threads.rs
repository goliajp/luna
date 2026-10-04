//! Threads: making (`lua_newthread`), resuming (`lua_resume` in each
//! dialect's shape), status, yieldability and closing.
//!
//! A resume from C leaves on the thread's stack what PUC leaves: the
//! yielded values on top of what the suspended frame kept, or the results
//! in place of the body and its arguments, or the error object twice on
//! top (PUC's `seterrorobj` at the top). A thread at its base level runs
//! the value under the arguments as its body (see `host_restartable`).

use super::ccall::{Cont, Wait, cstack, st};
use super::*;
use luna_core::runtime::CoroStatus;

const NON_SUSPENDED: &str = "cannot resume non-suspended coroutine";
const DEAD: &str = "cannot resume dead coroutine";
const OVERFLOW: &str = "C stack overflow";

/// A thread the C API has seen is being resumed: what the last resume from
/// C left above a yield from Lua code goes, and its frame comes back.
pub(super) fn thread_resuming(vm: &mut Vm, co: Gc<Coro>) {
    let l = state_of(vm, co);
    let s = st(l);
    if let Some((lo, base)) = s.parked.take() {
        cstack(co).truncate(lo);
        s.base = base;
    }
}

/// A thread was made: its state, with its copy of the main thread's extra
/// space, is made with it.
pub(super) fn thread_created(vm: &mut Vm, co: Gc<Coro>) {
    state_of(vm, co);
}

/// A thread the C API has seen was closed: its C frames and C stack go.
pub(super) fn thread_reset(vm: &mut Vm, co: Gc<Coro>) {
    let l = state_of(vm, co);
    let s = st(l);
    s.calls.clear();
    s.base = 0;
    s.parked = None;
    s.pending_yield = None;
    s.tbc.clear();
    cstack(co).clear();
}

/// Why a resume of `api`'s thread with `n` arguments is refused, in the
/// order each version checks.
fn refusal(api: &mut Api, n: usize) -> Option<&'static str> {
    let co = api.thread();
    let restartable = api.vm.host_restartable(co);
    let status = api.vm.host_coro_status(co);
    let running = matches!(status, CoroStatus::Running | CoroStatus::Normal);
    let dead = !restartable && status == CoroStatus::Dead;
    let full = api.vm.host_c_stack_full();
    match api.version() {
        LuaVersion::Lua51 if running || dead => Some(NON_SUSPENDED),
        LuaVersion::Lua51 => full.then_some(OVERFLOW),
        LuaVersion::Lua52 if full => Some(OVERFLOW),
        LuaVersion::Lua52 | LuaVersion::Lua53 if running => Some(NON_SUSPENDED),
        LuaVersion::Lua52 | LuaVersion::Lua53 if dead => Some(DEAD),
        LuaVersion::Lua52 | LuaVersion::Lua53 => full.then_some(OVERFLOW),
        _ if restartable && api.gettop() as usize == n => Some(DEAD),
        _ if running => Some(NON_SUSPENDED),
        _ if dead => Some(DEAD),
        _ => full.then_some(OVERFLOW),
    }
}

/// PUC `lua_resume` on `api`'s thread with its top `nargs` values: the
/// status, and for 5.4's `nresults` the count of values it reports (`None`
/// when a refused resume leaves it unset).
fn resume(api: &mut Api, nargs: c_int) -> (c_int, Option<c_int>) {
    let n = usize::try_from(nargs).unwrap_or(0);
    if let Some(msg) = refusal(api, n) {
        // 5.1 clears the frame, later versions remove the arguments
        if api.version() == LuaVersion::Lua51 {
            let base = api.base();
            api.truncate(base);
        } else {
            api.pop_n(n);
        }
        let m = api.str(msg.as_bytes());
        api.push(m);
        return (LUA_ERRRUN, None);
    }
    let co = api.thread();
    let args = api.pop_n(n);
    // a start: the body under the arguments stays where it was, as PUC
    // leaves it when the call fails
    let start = api
        .vm
        .host_restartable(co)
        .then(|| api.get(-1))
        .flatten()
        .map(|f| (f, args.clone()));
    let r = api.vm.host_resume(co, args);
    let v = api.version();
    let failed = api
        .vm
        .host_thread_error(co)
        .map(|k| super::ccall::status_code(v, k));
    match r {
        Ok(vals) if co.status == CoroStatus::Suspended => {
            let lo = api.top();
            let nres = vals.len() as c_int;
            api.push_all(&vals);
            let s = api.st();
            let c_frame = s
                .calls
                .last()
                .filter(|c| {
                    matches!(
                        c.cont,
                        Some(Cont {
                            wait: Wait::Yield,
                            ..
                        })
                    )
                })
                .map(|c| c.base);
            match c_frame {
                // 5.4 shows the yielding C function's whole frame
                Some(base) if v >= LuaVersion::Lua54 => s.base = base,
                Some(_) => s.base = lo,
                None => {
                    s.parked = Some((lo, s.base));
                    s.base = lo;
                }
            }
            (LUA_YIELD, Some(nres))
        }
        Ok(vals) => {
            api.push_all(&vals);
            (LUA_OK, Some(api.gettop()))
        }
        Err(e) => {
            if let Some((f, args)) = start {
                api.push(f);
                api.push_all(&args);
            }
            api.push(e.0);
            api.push(e.0);
            (failed.unwrap_or(LUA_ERRRUN), Some(api.gettop()))
        }
    }
}

/// PUC 5.4/5.5 `lua_resume`.
///
/// # Safety
/// `L` and `from` (null or not) are threads of the same open state, and no
/// API call on the state is running other than a C function it is calling
/// into; `nresults` is writable.
// SAFETY: no other item in the link is named `lua_resume`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_resume(
    L: *mut LuaState,
    _from: *mut LuaState,
    nargs: c_int,
    nresults: *mut c_int,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let (status, nres) = resume(&mut api, nargs);
    if let Some(n) = nres {
        // SAFETY: `nresults` is writable (# Safety)
        unsafe { *nresults = n };
    }
    status
}

/// PUC 5.2/5.3 `lua_resume`.
///
/// # Safety
/// As [`lua_resume`], without `nresults`.
// SAFETY: no other item in the link is named `luna_resume_52`: PUC's liblua
// has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_resume_52(
    L: *mut LuaState,
    _from: *mut LuaState,
    nargs: c_int,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    resume(&mut api, nargs).0
}

/// PUC 5.1 `lua_resume`.
///
/// # Safety
/// As [`lua_resume`], without `from` and `nresults`.
// SAFETY: no other item in the link is named `luna_resume_51`: PUC's liblua
// has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_resume_51(L: *mut LuaState, nargs: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    resume(&mut api, nargs).0
}

/// PUC `lua_newthread`: push a new thread and return its `lua_State`. It
/// shares `L`'s globals (5.1) and C hook, and its extra space starts as a
/// copy of the main thread's.
///
/// # Safety
/// `L` is a live thread of an open state, and no API call on it is running
/// other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_newthread`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_newthread(L: *mut LuaState) -> *mut LuaState {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let co = api.vm.host_new_thread();
    api.push(Value::Coro(co));
    let globals = api.thread_globals();
    api.vm.host_set_thread_globals(co, globals);
    let (func, mask, count) = {
        let h = &api.st().hook;
        (h.func, h.mask, h.count)
    };
    let l1 = state_of(api.vm, co);
    let h1 = &mut st(l1).hook;
    (h1.func, h1.mask, h1.count) = (func, mask, count);
    l1
}

/// PUC `lua_status`: `LUA_YIELD` while suspended, the error status of a
/// thread that died by an error, else `LUA_OK`.
///
/// # Safety
/// As [`lua_newthread`].
// SAFETY: no other item in the link is named `lua_status`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_status(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let api = unsafe { Api::new(L) };
    let co = api.thread();
    if api.vm.host_main_thread().ptr_eq(co) || api.vm.host_restartable(co) {
        return LUA_OK;
    }
    match co.status {
        CoroStatus::Suspended => LUA_YIELD,
        CoroStatus::Dead => api
            .vm
            .host_thread_error(co)
            .map_or(LUA_OK, |k| super::ccall::status_code(api.version(), k)),
        CoroStatus::Running | CoroStatus::Normal => LUA_OK,
    }
}

/// PUC `lua_isyieldable` (5.3+). A thread that is not running keeps what
/// its last resume left: 5.3 counts it non-yieldable, 5.4 yieldable unless
/// it is the main thread.
///
/// # Safety
/// As [`lua_newthread`].
// SAFETY: no other item in the link is named `lua_isyieldable`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isyieldable(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let api = unsafe { Api::new(L) };
    let co = api.thread();
    if api.vm.host_is_running(co) {
        return c_int::from(api.vm.host_is_yieldable(None));
    }
    let main = api.vm.host_main_thread().ptr_eq(co);
    c_int::from(api.version() >= LuaVersion::Lua54 && !main)
}

/// Close `api`'s thread (PUC `luaE_resetthread`): run its pending `__close`
/// handlers and clear its stack, leaving the error object there if it died
/// by one or a handler raised one. A thread closing itself (5.5's
/// `lua_closethread(L, L)`) ends the resume that runs it instead.
fn close(api: &mut Api) -> c_int {
    let co = api.thread();
    if api.vm.host_main_thread().ptr_eq(co) {
        // outside a C function the main thread's stack holds what the host
        // pushed: its to-be-closed slots are closed, newest first, each
        // getting the error the one before raised
        if !api.st().calls.is_empty() {
            return LUA_OK;
        }
        let before = api.vm.special_errors();
        let r = super::tbc::close_with(api, 0, None);
        api.truncate(0);
        return match r {
            Ok(()) => LUA_OK,
            Err(LuaError(e)) => {
                api.push(e);
                let kind = api.vm.error_status(e, before);
                super::ccall::status_code(api.version(), kind)
            }
        };
    }
    if api.vm.host_is_running(co) {
        let e = api.vm.host_close_running();
        api.raise(e);
        return LUA_OK;
    }
    match api.vm.host_close_thread(co) {
        Ok(None) => LUA_OK,
        Ok(Some(e)) | Err(LuaError(e)) => {
            api.push(e);
            LUA_ERRRUN
        }
    }
}

/// PUC `lua_closethread` (5.4.6+).
///
/// # Safety
/// As [`lua_resume`]; called by the C wrapper, which throws what this
/// raises.
// SAFETY: no other item in the link is named `luna_capi_lua_closethread`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_closethread(
    L: *mut LuaState,
    _from: *mut LuaState,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    close(&mut api)
}

/// PUC 5.4 `lua_resetthread`, `lua_closethread(L, NULL)`.
///
/// # Safety
/// As [`luna_capi_lua_closethread`].
// SAFETY: no other item in the link is named `luna_capi_lua_resetthread`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_resetthread(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    close(&mut api)
}

c_exports! {
    lua_closethread => luna_c_lua_closethread,
    lua_resetthread => luna_c_lua_resetthread,
}
