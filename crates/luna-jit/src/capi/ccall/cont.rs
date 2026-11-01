//! Suspending a C function that yields, and running or dropping its
//! continuation when its coroutine is resumed or collected.

use super::*;

/// The C function of call `token` yielded `py.n` values: suspend the coroutine. Its call stays
/// until the resume, which runs its continuation or, without one, returns the resume's values from
/// it; until then its frame stays on the C stack below the yielded values, as PUC keeps it.
pub(super) fn yield_from_c(
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
    let vals = cstack(co)[from..].to_vec();
    cstack(co).truncate(from);
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
pub(super) fn run_cont(
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
                let (fs, base) = {
                    let c = &st(l).calls[token];
                    (c.func_slot, c.base)
                };
                // the resume's values may be held by nothing else: no
                // emergency collection runs while they go on the C stack
                if let Err(o) = cstack(co).extend_from_slice(&vals) {
                    pop_call(l, co, token);
                    return Err(o.into());
                }
                // SAFETY: `co` is a live thread of this Vm; the heap is written in place
                unsafe { vm.heap_mut().barrier_back(co) };
                let r = close_frame(vm, l, token, None)
                    .and_then(|()| return_hook(vm, co, base, vals.len()))
                    .and_then(|()| vm.nat_ret(fs, &vals));
                pop_call(l, co, token);
                return r;
            }
            if let Err(o) = cstack(co).extend_from_slice(&vals) {
                pop_call(l, co, token);
                return Err(o.into());
            }
            LUA_YIELD
        }
        Wait::Call { nresults, at } => {
            push_results(vm, co, at, vals, nresults)?;
            LUA_YIELD
        }
        Wait::PCall {
            nresults,
            at,
            errerr_before,
        } => pcall_outcome(vm, co, vals, nresults, at, errerr_before).unwrap_or(LUA_YIELD),
        Wait::Recover { status } => status,
    };
    // SAFETY: `co` is a live thread of this Vm; the heap is written in place
    unsafe { vm.heap_mut().barrier_back(co) };
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

/// An error left the C function of call `token`, whose continuation will
/// never run.
fn cont_discard(vm: &mut Vm, co: Gc<Coro>, token: u32) {
    let l = state_of(vm, co);
    pop_call(l, co, token as usize);
}

/// The C API's side of `ContKind::Host` continuations.
pub(in crate::capi) const CONT_HOOKS: HostContHooks = HostContHooks {
    resume: cont_resume,
    discard: cont_discard,
    resuming: super::threads::thread_resuming,
    reset: super::threads::thread_reset,
    created: super::threads::thread_created,
};
