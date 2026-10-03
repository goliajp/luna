//! Calling C functions: the native that runs one under an error boundary,
//! its frame on the thread's C stack, how it leaves (returning, raising or
//! yielding), and running its continuation after a yield.

use super::*;
use luna_core::runtime::NativeClosure;
use luna_core::vm::exec::host_c::HostContHooks;

/// PUC `lua_Hook`.
pub type LuaHook = extern "C" fn(*mut LuaState, *mut c_void);

/// A continuation function: 5.3+ `lua_KFunction`, or 5.2's, which is a
/// `lua_CFunction` that asks `lua_getctx` for its status and context.
#[derive(Clone, Copy)]
pub(super) enum KFn {
    K53(LuaKFunction),
    K52(LuaCFunction),
}

/// What a continuation waits for.
#[derive(Clone, Copy)]
pub(super) enum Wait {
    /// the call `lua_callk` made, wanting `nresults`
    Call { nresults: c_int },
    /// the protected call `lua_pcallk` made: its results go on the stack
    /// from `at`, an error replaces everything from `at` on
    PCall {
        nresults: c_int,
        at: usize,
        errerr_before: u64,
    },
    /// the resume of the coroutine `lua_yieldk` suspended
    Yield,
    /// nothing: the protected call `lua_pcallk` made in a coroutine failed
    /// before any yield, and, as in PUC, the C function is left and its
    /// continuation runs with this error status
    Recover { status: c_int },
}

/// The status a C wrapper throws to leave a C function whose continuation
/// runs at once (`Wait::Recover`).
pub(super) const LUNA_RECOVER: c_int = -2;

/// A continuation a C function left behind (`lua_callk`, `lua_pcallk`,
/// `lua_yieldk`).
#[derive(Clone, Copy)]
pub(super) struct Cont {
    /// `None` for a `lua_yield` without one: the resume's values are then
    /// the C function's results
    pub(super) k: Option<KFn>,
    pub(super) ctx: isize,
    pub(super) wait: Wait,
}

/// One C function running on a thread (PUC's `CallInfo` of a C function).
pub(super) struct CCall {
    /// its index 1 in the thread's C stack
    pub(super) base: usize,
    /// the base of the frame below, put back when it leaves
    pub(super) outer_base: usize,
    /// its slot on the Lua stack, where its results go
    pub(super) func_slot: u32,
    /// results its caller wants
    pub(super) nresults: i32,
    /// its closure, for its upvalues; rooted by `func_slot`
    pub(super) nc: Gc<NativeClosure>,
    /// what a yield inside will resume with
    pub(super) cont: Option<Cont>,
    /// 5.2 `lua_getctx`: the status its continuation was called with
    pub(super) k_status: c_int,
    /// 5.2 `lua_getctx`: the context the last `lua_callk`, `lua_pcallk` or
    /// `lua_yieldk` with a continuation gave
    pub(super) k_ctx: isize,
}

/// A `lua_yield` the running C function made, acted on when it leaves.
#[derive(Clone, Copy)]
pub(super) struct PendingYield {
    pub(super) n: c_int,
    pub(super) k: Option<KFn>,
    pub(super) ctx: isize,
}

/// A thread's C hook (`lua_sethook`) and its debug-interface bookkeeping.
#[derive(Default)]
pub(super) struct CHook {
    /// the hook `lua_sethook` installed, with its mask and count as given
    pub(super) func: Option<LuaHook>,
    pub(super) mask: c_int,
    pub(super) count: c_int,
    /// the C hook running on this thread now
    pub(super) running: Option<super::hooks::HookRun>,
    /// the strings `lua_getinfo` and `lua_getlocal` handed out
    pub(super) strs: super::debug::CStrings,
}

// SAFETY: the declarations match the definitions in `csrc/shim_core.c`;
// each runs a host function under a fresh error boundary and returns
// normally, whatever the host function does
// C sees `lua_State` as opaque and reads only its leading fields, which
// `csrc/shim.h` declares
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn luna_c_protect(L: *mut LuaState, f: LuaCFunction, nret: *mut c_int) -> c_int;
    fn luna_c_protect_k(
        L: *mut LuaState,
        k: LuaKFunction,
        status: c_int,
        ctx: isize,
        nret: *mut c_int,
    ) -> c_int;
}

/// Run `f` with the state's Vm pointer set to one derived from `vm`, so
/// that what C does to the Vm while `f` runs goes through `vm`, and put the
/// previous pointer back after.
pub(super) fn with_c<R>(vm: &mut Vm, l: *mut LuaState, f: impl FnOnce() -> R) -> R {
    let vm_ptr: *mut Vm = vm;
    // SAFETY: `l` is a live thread of this Vm's state, so its global record
    // is live; the field is a plain pointer
    let prev = unsafe { std::mem::replace(&mut (*(*l).g).vm, vm_ptr) };
    let r = f();
    // SAFETY: as above
    unsafe { (*(*l).g).vm = prev };
    r
}

/// The C stack of `co`.
pub(super) fn cstack<'a>(co: Gc<Coro>) -> &'a mut Vec<Value> {
    // SAFETY: the thread is live while its state is; the C stack is only
    // reached through short-lived borrows like this one, none of which
    // overlaps a call that could reach C
    unsafe { &mut (*co.as_ptr()).host_stack }
}

/// The state record behind `l`.
pub(super) fn st<'a>(l: *mut LuaState) -> &'a mut LuaState {
    // SAFETY: `l` is a live thread of the state; the borrows made through
    // this end before any call that could reach C
    unsafe { &mut *l }
}

/// The native every C function is wrapped in: its upvalue 0 holds the C
/// function's pointer. It copies the arguments to a new frame on the
/// thread's C stack and calls the C function under an error boundary.
pub(super) fn capi_trampoline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let nresults = vm.host_native_nresults();
    let Value::LightUserdata(p) = vm.running_native_upvalue(0) else {
        unreachable!("a C function's native keeps its pointer");
    };
    // SAFETY: upvalue 0 of this native was set from a `lua_CFunction` by
    // `lua_pushcclosure`, and a function pointer round-trips through a
    // data pointer on every target luna supports
    let cf: LuaCFunction = unsafe { std::mem::transmute::<*const (), LuaCFunction>(p) };
    let nc = vm.host_running_native().expect("the trampoline is running");
    let co = vm.host_thread();
    let l = state_of(vm, co);
    let args: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let base = cstack(co).len();
    cstack(co).extend_from_slice(&args);
    vm.heap.barrier_back(co);
    let s = st(l);
    s.calls.push(CCall {
        base,
        outer_base: s.base,
        func_slot: fs,
        nresults,
        nc,
        cont: None,
        k_status: LUA_OK,
        k_ctx: 0,
    });
    s.base = base;
    let token = s.calls.len() - 1;
    let mut nret: c_int = 0;
    // SAFETY: `l` is a live thread and `cf` a C function the host gave
    // `lua_pushcclosure`; the boundary returns whatever the function does
    let status = with_c(vm, l, || unsafe { luna_c_protect(l, cf, &mut nret) });
    leave(vm, l, co, token, status, nret)
}

/// Take C call `token` and everything above it off the thread: its values
/// leave the C stack and the frame below becomes the current one.
pub(super) fn pop_call(l: *mut LuaState, co: Gc<Coro>, token: usize) {
    let s = st(l);
    let Some(c) = s.calls.get(token) else {
        return;
    };
    let (base, outer) = (c.base, c.outer_base);
    s.calls.truncate(token);
    s.base = outer;
    s.tbc.retain(|&i| i < base);
    cstack(co).truncate(base);
}

/// Close the to-be-closed slots of call `token`'s frame as it leaves,
/// passing the error it leaves with (`None`: it returns); an error a
/// `__close` raises replaces that.
fn close_frame(
    vm: &mut Vm,
    l: *mut LuaState,
    token: usize,
    err: Option<Value>,
) -> Result<(), LuaError> {
    let base = st(l).calls[token].base;
    if !st(l).tbc.last().is_some_and(|&i| i >= base) {
        return Ok(());
    }
    let mut api = Api { vm, l };
    super::tbc::close_with(&mut api, base, err)
}

/// The C function of call `token` came back from C with `status`: finish
/// its native's call with what it returned, or pass on its error or
/// yield.
fn leave(
    vm: &mut Vm,
    l: *mut LuaState,
    co: Gc<Coro>,
    token: usize,
    status: c_int,
    nret: c_int,
) -> Result<u32, LuaError> {
    let (fs, nresults) = {
        let c = &st(l).calls[token];
        (c.func_slot, c.nresults)
    };
    if status == LUNA_RECOVER {
        return run_cont(vm, l, co, token, Vec::new());
    }
    if status == LUA_OK || status == LUA_YIELD {
        if let Some(py) = st(l).pending_yield.take() {
            return yield_from_c(vm, l, co, token, py);
        }
        if status == LUA_YIELD {
            // a call it made with a continuation yielded: the coroutine is
            // suspending, and the call record stays for the resume
            debug_assert!(vm.host_yielding());
            return Err(LuaError(Value::Nil));
        }
        let n = usize::try_from(nret).unwrap_or(0);
        let base = st(l).calls[token].base;
        let top = cstack(co).len();
        if n > top - base {
            pop_call(l, co, token);
            let s = Value::Str(
                vm.heap
                    .intern(b"C function returned more values than were pushed"),
            );
            return Err(LuaError(s));
        }
        let results: Vec<Value> = cstack(co)[top - n..].to_vec();
        let closed = close_frame(vm, l, token, None);
        pop_call(l, co, token);
        let _ = nresults;
        closed?;
        return Ok(vm.nat_return(fs, &results));
    }
    let err = take_error(l);
    let closed = close_frame(vm, l, token, Some(err));
    pop_call(l, co, token);
    closed?;
    Err(LuaError(err))
}

/// The error object being thrown: the top of the thread that raised it.
pub(super) fn take_error(l: *mut LuaState) -> Value {
    // SAFETY: `l` is live, so is its global record
    let from = unsafe { std::mem::replace(&mut (*(*l).g).err_from, std::ptr::null_mut()) };
    let from = if from.is_null() { l } else { from };
    cstack(st(from).thread).pop().unwrap_or(Value::Nil)
}

/// The C function of call `token` yielded `py.n` values: suspend the
/// coroutine. Its call stays until the resume, which runs its continuation
/// or, without one, returns the resume's values from it; until then its
/// frame stays on the C stack below the yielded values, as PUC keeps it.
fn yield_from_c(
    vm: &mut Vm,
    l: *mut LuaState,
    co: Gc<Coro>,
    token: usize,
    py: PendingYield,
) -> Result<u32, LuaError> {
    let (fs, nresults, base) = {
        let c = &st(l).calls[token];
        (c.func_slot, c.nresults, c.base)
    };
    let n = usize::try_from(py.n).unwrap_or(0);
    let from = cstack(co).len().saturating_sub(n).max(base);
    let vals = cstack(co).split_off(from);
    let c = &mut st(l).calls[token];
    c.cont = Some(Cont {
        k: py.k,
        ctx: py.ctx,
        wait: Wait::Yield,
    });
    if py.k.is_some() {
        c.k_ctx = py.ctx;
    }
    Err(vm.host_yield(fs, nresults, vals, Some(token as u32)))
}

/// Run the continuation of call `token` of thread `co`: its frame comes
/// back with `vals` on top (the call's results, or the resume's values),
/// and the continuation is called under a fresh boundary.
fn cont_resume(
    vm: &mut Vm,
    co: Gc<Coro>,
    token: u32,
    _fs: u32,
    vals: Vec<Value>,
) -> Result<u32, LuaError> {
    let l = state_of(vm, co);
    let token = token as usize;
    let s = st(l);
    s.calls.truncate(token + 1);
    s.base = s.calls[token].base;
    run_cont(vm, l, co, token, vals)
}

/// Call the continuation call `token` waits on, with `vals` as what it
/// waited for, and leave the C function with what the continuation does.
fn run_cont(
    vm: &mut Vm,
    l: *mut LuaState,
    co: Gc<Coro>,
    token: usize,
    vals: Vec<Value>,
) -> Result<u32, LuaError> {
    let cont = st(l).calls[token]
        .cont
        .take()
        .expect("a continuation to run");
    let status = match cont.wait {
        Wait::Yield => {
            if cont.k.is_none() {
                let fs = st(l).calls[token].func_slot;
                let closed = close_frame(vm, l, token, None);
                pop_call(l, co, token);
                closed?;
                return Ok(vm.nat_return(fs, &vals));
            }
            cstack(co).extend_from_slice(&vals);
            LUA_YIELD
        }
        Wait::Call { nresults } => {
            push_results(co, vals, nresults);
            LUA_YIELD
        }
        Wait::PCall {
            nresults,
            at,
            errerr_before,
        } => pcall_outcome(vm, co, vals, nresults, at, errerr_before).unwrap_or(LUA_YIELD),
        Wait::Recover { status } => status,
    };
    vm.heap.barrier_back(co);
    st(l).calls[token].k_status = status;
    let mut nret: c_int = 0;
    let ret = match cont.k.expect("a continuation function") {
        // SAFETY: `l` is a live thread and `k` the continuation the host
        // gave `lua_callk`/`lua_pcallk`/`lua_yieldk`
        KFn::K53(k) => with_c(vm, l, || unsafe {
            luna_c_protect_k(l, k, status, cont.ctx, &mut nret)
        }),
        // SAFETY: as above, for 5.2's continuation shape
        KFn::K52(k) => with_c(vm, l, || unsafe { luna_c_protect(l, k, &mut nret) }),
    };
    leave(vm, l, co, token, ret, nret)
}

/// Put a call's results on the stack, adjusted to `nresults` (-1: all).
pub(super) fn push_results(co: Gc<Coro>, mut vals: Vec<Value>, nresults: c_int) {
    if nresults >= 0 {
        vals.resize(nresults as usize, Value::Nil);
    }
    cstack(co).extend(vals);
}

/// What a protected call's `true, results...` or `false, error` leaves on
/// the stack from `at`: `None` for success, else the error status.
pub(super) fn pcall_outcome(
    vm: &mut Vm,
    co: Gc<Coro>,
    mut vals: Vec<Value>,
    nresults: c_int,
    at: usize,
    errerr_before: u64,
) -> Option<c_int> {
    let ok = vals.first().is_some_and(|v| v.truthy());
    if ok {
        vals.remove(0);
        push_results(co, vals, nresults);
        return None;
    }
    let err = vals.get(1).copied().unwrap_or(Value::Nil);
    cstack(co).truncate(at);
    cstack(co).push(err);
    let errerr = vm.host_errerr_count() != errerr_before
        && matches!(err, Value::Str(s) if s.as_bytes() == b"error in error handling");
    Some(match (errerr, vm.version()) {
        (false, _) => LUA_ERRRUN,
        (true, LuaVersion::Lua52 | LuaVersion::Lua53) => 6,
        (true, _) => LUA_ERRERR,
    })
}

/// An error left the C function of call `token`, whose continuation will
/// never run.
fn cont_discard(vm: &mut Vm, co: Gc<Coro>, token: u32) {
    let l = state_of(vm, co);
    pop_call(l, co, token as usize);
}

/// The C API's side of `ContKind::Host` continuations.
pub(super) const CONT_HOOKS: HostContHooks = HostContHooks {
    resume: cont_resume,
    discard: cont_discard,
    resuming: super::threads::thread_resuming,
    reset: super::threads::thread_reset,
};
