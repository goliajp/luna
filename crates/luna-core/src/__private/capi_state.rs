//! The C API's view of a Vm: threads, coroutines, calls and yields.

use crate::runtime::*;
use crate::vm::LuaError;
use crate::vm::exec::host_c::*;
use crate::vm::exec::*;

/// What the C API (`luna-jit`'s `capi`) asks of a Vm: threads, coroutines, calls and yields.
pub trait VmCApiState {
    /// Install the C API's continuation hooks.
    fn set_host_cont_hooks(&mut self, hooks: HostContHooks);
    /// Install a warning function, or put the default back (`None`).
    fn set_host_warn(&mut self, warn: Option<HostWarn>);
    /// The running thread: a coroutine, or the main thread's identity.
    fn host_thread(&self) -> Gc<Coro>;
    /// The main thread's identity object.
    fn host_main_thread(&self) -> Gc<Coro>;
    /// Whether `co` is the thread that is running.
    fn host_is_running(&self, co: Gc<Coro>) -> bool;
    /// A new thread with no function yet (PUC `lua_newthread`): the first
    /// resume takes the function from the thread's C stack.
    fn host_new_thread(&mut self) -> Result<Gc<Coro>, LuaError>;
    /// A value's text without metamethods, as `lua_tolstring` converts a
    /// number (`tostring` of other values).
    fn host_basic_text(&mut self, v: Value) -> Vec<u8>;
    /// The number a string converts to, as the dialect converts it (PUC
    /// `cvt2num` / `lua_stringtonumber`).
    fn host_str_to_number(&self, s: &[u8]) -> Option<Value>;
    /// The function the C API made for the light C function `key`, made
    /// with `make` the first time (5.2+'s functions without upvalues, which
    /// PUC compares by their C pointer), uncounted. It lives as long as the Vm.
    fn host_light_fn(
        &mut self,
        key: usize,
        make: impl FnOnce(&mut Vm) -> Result<Value, LuaError>,
    ) -> Result<Value, LuaError>;
    /// The native running on top of the dispatch chain.
    fn host_running_native(&self) -> Option<Gc<crate::runtime::NativeClosure>>;
    /// The globals of thread `co` (5.1 `LUA_GLOBALSINDEX`).
    fn host_thread_globals(&self, co: Gc<Coro>) -> Gc<Table>;
    /// Replace the globals of thread `co` (5.1 `lua_replace` into
    /// `LUA_GLOBALSINDEX`).
    fn host_set_thread_globals(&mut self, co: Gc<Coro>, t: Gc<Table>);
    /// Results the call of the running native wants (-1 for all), as its
    /// caller set them; read at the native's entry.
    fn host_native_nresults(&self) -> i32;
    /// Why the running thread cannot yield now, as PUC words it, or `None`.
    fn host_yield_refusal(&self) -> Option<&'static str>;
    /// PUC `lua_isyieldable` of `co` (the running thread when `None`).
    fn host_is_yieldable(&self, co: Option<Gc<Coro>>) -> bool;
    /// Whether a yield is on its way out (the `Err` a call returned is the
    /// yield, not an error).
    fn host_yielding(&self) -> bool;
    /// Call `f(args)` from a C function (PUC `lua_callk`). With `cont` and
    /// a thread that can yield, a yield inside `f` suspends the coroutine
    /// with the continuation on the frame stack: the `Err` that comes back
    /// is then the yield (`Vm::host_yielding`), and the C function must
    /// leave. Without `cont` the call cannot yield.
    fn host_call(
        &mut self,
        f: Value,
        args: &[Value],
        cont: Option<HostContSpec>,
    ) -> Result<Vec<Value>, LuaError>;
    /// Suspend the running coroutine from the C function at `func_slot`
    /// (PUC `lua_yieldk`), yielding `vals`. With `cont`, the resume runs
    /// the continuation; without, the resume's values are the C function's
    /// results. The caller has checked `Vm::host_yield_refusal`.
    fn host_yield(
        &mut self,
        func_slot: u32,
        nresults: i32,
        vals: Vec<Value>,
        cont: Option<u32>,
    ) -> LuaError;
    /// Resume `co` with `args` (PUC `lua_resume`). `Ok` carries what it
    /// yielded or returned (its status tells which); `Err` the error that
    /// killed it.
    fn host_resume(&mut self, co: Gc<Coro>, args: Vec<Value>) -> Result<Vec<Value>, LuaError>;
    /// The function a C API protected call (`lua_pcallk`) calls, which is
    /// not a level of the stack: with a handler it takes `(f, msgh,
    /// args...)`, without `(f, args...)`; it returns `true, results...` or
    /// `false, error`.
    fn host_protected_fn(&mut self, handler: bool) -> Result<Value, LuaError>;
    /// Fire the return hook of the running C function now, while its own
    /// stack still holds its arguments, what it pushed and its `n` results,
    /// the first of them at transfer index `ftransfer` (PUC `rethook` runs
    /// before the results move); its return then skips the hook.
    fn host_c_return_hook(&mut self, ftransfer: u32, n: u32) -> Result<(), LuaError>;
    /// The status `co` died with, when it died by an error and was not
    /// closed since.
    fn host_thread_error(&self, co: Gc<Coro>) -> Option<crate::runtime::ErrorStatus>;
    /// Close `co` (PUC `lua_closethread`): run its pending `__close`
    /// handlers and mark it dead. `Ok(Some(e))`: it had died with `e`.
    fn host_close_thread(&mut self, co: Gc<Coro>) -> Result<Option<Value>, LuaError>;
    /// PUC `lua_close` before the state is freed: close the main thread's
    /// pending to-be-closed variables and run every finalizer, dropping
    /// their errors.
    fn host_close_state(&mut self);
    /// PUC's registry (`LUA_REGISTRYINDEX`), made now if it was not
    /// (`Vm::host_make_registry`), ending the process if it cannot be.
    fn host_registry(&mut self) -> Gc<Table>;
    /// Make the registry (PUC `init_registry`) with the entries
    /// `lua_newstate` puts in it: from 5.2 on the main thread and the
    /// globals (5.5 also `false` at index 1, for `luaL_ref`).
    fn host_make_registry(&mut self) -> Result<Gc<Table>, LuaError>;
    /// Whether `co` sits at its base level with its next body on its C
    /// stack: a thread the C API has seen that has not started, returned,
    /// or was closed.
    fn host_restartable(&self, co: Gc<Coro>) -> bool;
    /// The status of `co` as `coroutine.status` reports it.
    fn host_coro_status(&self, co: Gc<Coro>) -> CoroStatus;
    /// Whether a resume now would overflow PUC's C stack
    /// (`LUAI_MAXCCALLS`).
    fn host_c_stack_full(&self) -> bool;
    /// Whether the running coroutine is closing itself (the `Err` a call
    /// returned is that, not an error).
    fn host_terminating(&self) -> bool;
    /// Close the running coroutine from inside it (PUC 5.5
    /// `lua_closethread(L, L)`): run its pending `__close` handlers; the
    /// returned value is what the C function leaves with, and the resume
    /// that runs the coroutine then ends it.
    fn host_close_running(&mut self) -> LuaError;
}

impl VmCApiState for crate::vm::Vm {
    #[inline]
    fn set_host_cont_hooks(&mut self, hooks: HostContHooks) {
        crate::vm::Vm::set_host_cont_hooks(self, hooks)
    }
    #[inline]
    fn set_host_warn(&mut self, warn: Option<HostWarn>) {
        crate::vm::Vm::set_host_warn(self, warn)
    }
    #[inline]
    fn host_thread(&self) -> Gc<Coro> {
        crate::vm::Vm::host_thread(self)
    }
    #[inline]
    fn host_main_thread(&self) -> Gc<Coro> {
        crate::vm::Vm::host_main_thread(self)
    }
    #[inline]
    fn host_is_running(&self, co: Gc<Coro>) -> bool {
        crate::vm::Vm::host_is_running(self, co)
    }
    #[inline]
    fn host_new_thread(&mut self) -> Result<Gc<Coro>, LuaError> {
        crate::vm::Vm::host_new_thread(self)
    }
    #[inline]
    fn host_basic_text(&mut self, v: Value) -> Vec<u8> {
        crate::vm::Vm::host_basic_text(self, v)
    }
    #[inline]
    fn host_str_to_number(&self, s: &[u8]) -> Option<Value> {
        crate::vm::Vm::host_str_to_number(self, s)
    }
    #[inline]
    fn host_light_fn(
        &mut self,
        key: usize,
        make: impl FnOnce(&mut Vm) -> Result<Value, LuaError>,
    ) -> Result<Value, LuaError> {
        crate::vm::Vm::host_light_fn(self, key, make)
    }
    #[inline]
    fn host_running_native(&self) -> Option<Gc<crate::runtime::NativeClosure>> {
        crate::vm::Vm::host_running_native(self)
    }
    #[inline]
    fn host_thread_globals(&self, co: Gc<Coro>) -> Gc<Table> {
        crate::vm::Vm::host_thread_globals(self, co)
    }
    #[inline]
    fn host_set_thread_globals(&mut self, co: Gc<Coro>, t: Gc<Table>) {
        crate::vm::Vm::host_set_thread_globals(self, co, t)
    }
    #[inline]
    fn host_native_nresults(&self) -> i32 {
        crate::vm::Vm::host_native_nresults(self)
    }
    #[inline]
    fn host_yield_refusal(&self) -> Option<&'static str> {
        crate::vm::Vm::host_yield_refusal(self)
    }
    #[inline]
    fn host_is_yieldable(&self, co: Option<Gc<Coro>>) -> bool {
        crate::vm::Vm::host_is_yieldable(self, co)
    }
    #[inline]
    fn host_yielding(&self) -> bool {
        crate::vm::Vm::host_yielding(self)
    }
    #[inline]
    fn host_call(
        &mut self,
        f: Value,
        args: &[Value],
        cont: Option<HostContSpec>,
    ) -> Result<Vec<Value>, LuaError> {
        crate::vm::Vm::host_call(self, f, args, cont)
    }
    #[inline]
    fn host_yield(
        &mut self,
        func_slot: u32,
        nresults: i32,
        vals: Vec<Value>,
        cont: Option<u32>,
    ) -> LuaError {
        crate::vm::Vm::host_yield(self, func_slot, nresults, vals, cont)
    }
    #[inline]
    fn host_resume(&mut self, co: Gc<Coro>, args: Vec<Value>) -> Result<Vec<Value>, LuaError> {
        crate::vm::Vm::host_resume(self, co, args)
    }
    #[inline]
    fn host_protected_fn(&mut self, handler: bool) -> Result<Value, LuaError> {
        crate::vm::Vm::host_protected_fn(self, handler)
    }
    #[inline]
    fn host_c_return_hook(&mut self, ftransfer: u32, n: u32) -> Result<(), LuaError> {
        crate::vm::Vm::host_c_return_hook(self, ftransfer, n)
    }
    #[inline]
    fn host_thread_error(&self, co: Gc<Coro>) -> Option<crate::runtime::ErrorStatus> {
        crate::vm::Vm::host_thread_error(self, co)
    }
    #[inline]
    fn host_close_thread(&mut self, co: Gc<Coro>) -> Result<Option<Value>, LuaError> {
        crate::vm::Vm::host_close_thread(self, co)
    }
    #[inline]
    fn host_close_state(&mut self) {
        crate::vm::Vm::host_close_state(self)
    }
    #[inline]
    fn host_registry(&mut self) -> Gc<Table> {
        crate::vm::Vm::host_registry(self)
    }
    #[inline]
    fn host_make_registry(&mut self) -> Result<Gc<Table>, LuaError> {
        crate::vm::Vm::host_make_registry(self)
    }
    #[inline]
    fn host_restartable(&self, co: Gc<Coro>) -> bool {
        crate::vm::Vm::host_restartable(self, co)
    }
    #[inline]
    fn host_coro_status(&self, co: Gc<Coro>) -> CoroStatus {
        crate::vm::Vm::host_coro_status(self, co)
    }
    #[inline]
    fn host_c_stack_full(&self) -> bool {
        crate::vm::Vm::host_c_stack_full(self)
    }
    #[inline]
    fn host_terminating(&self) -> bool {
        crate::vm::Vm::host_terminating(self)
    }
    #[inline]
    fn host_close_running(&mut self) -> LuaError {
        crate::vm::Vm::host_close_running(self)
    }
}
