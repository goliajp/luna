//! What `luna-jit`'s C API needs from the VM beyond the embedding API:
//! threads as `lua_State`s, calls a C function makes with a continuation
//! (`lua_callk`, `lua_pcallk`, `lua_yieldk`), and the C library's hooks
//! into warnings. Everything here is hidden from the documentation; the C
//! API is its only user.
//!
//! A C function's continuation lives on the frame stack as a
//! [`ContKind::Host`] frame once a yield has left the C function. The VM
//! does not know what a continuation is: it calls back into the C API
//! through [`HostContHooks`] when the frame's call returns (or the yield is
//! resumed), and when an error drops the frame.

use super::*;
use crate::vm::callstack::NativeAct;

mod alloc;
mod block;
mod cont;
pub(crate) mod debug;
pub(crate) mod libs;
pub(crate) mod load;
mod registry;
mod threads;
mod upvals;
pub(crate) mod values;
pub use block::HostBlock;
pub(crate) use debug::{HostAr, HostHookFn, HostLevel};
pub(crate) use libs::HostOpenError;
pub(crate) use load::HostLoadError;

/// The C API's side of a [`ContKind::Host`] continuation.
#[derive(Clone, Copy)]
pub struct HostContHooks {
    /// Run continuation `token` of `thread`, whose C function sits at
    /// `func_slot`, with `values` (the results of the call it made, or what
    /// the resume passed): write the C function's results from `func_slot`
    /// on and return how many there are. A yield or an error the
    /// continuation raises comes back as the `Err`.
    pub resume: fn(&mut Vm, Gc<Coro>, u32, u32, Vec<Value>) -> Result<u32, LuaError>,
    /// Continuation `token` of `thread` will never run: an error left it.
    pub discard: fn(&mut Vm, Gc<Coro>, u32),
    /// A thread the C API has seen is about to be resumed.
    pub resuming: fn(&mut Vm, Gc<Coro>),
    /// A thread the C API has seen was closed: its C frames and stack go.
    pub reset: fn(&mut Vm, Gc<Coro>),
    /// A thread was made (`lua_newthread`): it copies the main thread's extra space.
    pub created: fn(&mut Vm, Gc<Coro>),
}

/// A warning function that replaces the default one (PUC `lua_setwarnf`):
/// called with the Vm, each piece of a warning and whether more pieces
/// follow. An error it raises leaves the call that warned.
pub type HostWarn = Box<dyn FnMut(&mut Vm, &[u8], bool) -> Result<(), LuaError>>;

/// The continuation a C function keeps while it calls a function or
/// yields: see `Vm::host_call`.
#[derive(Clone, Copy)]
pub struct HostContSpec {
    /// stack slot of the running C function
    pub func_slot: u32,
    /// results the C function's caller wants (-1 for all)
    pub nresults: i32,
    /// the C API's index for the continuation
    pub token: u32,
}

impl Vm {
    /// Install the C API's continuation hooks.
    pub(crate) fn set_host_cont_hooks(&mut self, hooks: HostContHooks) {
        self.host_cont_hooks = Some(hooks);
    }

    /// Install a warning function, or put the default back (`None`).
    pub(crate) fn set_host_warn(&mut self, warn: Option<HostWarn>) {
        self.host_warn = warn;
    }

    /// The running thread: a coroutine, or the main thread's identity.
    pub(crate) fn host_thread(&self) -> Gc<Coro> {
        self.current
            .unwrap_or_else(|| self.main_coro.expect("main thread"))
    }

    /// The main thread's identity object.
    pub(crate) fn host_main_thread(&self) -> Gc<Coro> {
        self.main_coro.expect("main thread")
    }

    /// Whether `co` is the thread that is running.
    pub(crate) fn host_is_running(&self, co: Gc<Coro>) -> bool {
        self.host_thread().ptr_eq(co)
    }

    /// A new thread with no function yet (PUC `lua_newthread`): the first
    /// resume takes the function from the thread's C stack.
    pub(crate) fn host_new_thread(&mut self) -> Result<Gc<Coro>, LuaError> {
        self.native_checkpoint()?;
        let globals = self.globals;
        let co = self.retry_oom(|vm| vm.heap.try_new_coro(Value::Nil, globals))?;
        // a new thread inherits the creator's C hook, as `new_coro` gives it
        if matches!(self.hook.func, Some(Value::LightUserdata(_))) {
            let mut h = self.hook;
            h.rust_func = None;
            h.count_left = h.count_base;
            // SAFETY: `co` was just allocated and nothing else refers to it
            unsafe { co.as_mut() }.hook = h;
        }
        Ok(co)
    }

    /// A value's text without metamethods, as `lua_tolstring` converts a
    /// number (`tostring` of other values).
    pub(crate) fn host_basic_text(&mut self, v: Value) -> Vec<u8> {
        self.tostring_basic(v)
    }

    /// The number a string converts to, as the dialect converts it (PUC
    /// `cvt2num` / `lua_stringtonumber`).
    pub(crate) fn host_str_to_number(&self, s: &[u8]) -> Option<Value> {
        Some(match str_to_num(s, self.version)? {
            numeric::Num::Int(i) => Value::Int(i),
            numeric::Num::Float(f) => Value::Float(f),
        })
    }

    /// The function the C API made for the light C function `key`, made
    /// with `make` the first time (5.2+'s functions without upvalues, which
    /// PUC compares by their C pointer), uncounted. It lives as long as the Vm.
    pub(crate) fn host_light_fn(
        &mut self,
        key: usize,
        make: impl FnOnce(&mut Vm) -> Result<Value, LuaError>,
    ) -> Result<Value, LuaError> {
        if let Some(&v) = self.host_light.get(&key) {
            return Ok(v);
        }
        let v = self.uncounted(make)?;
        self.host_light.insert(key, v);
        Ok(v)
    }

    /// The native running on top of the dispatch chain.
    pub(crate) fn host_running_native(&self) -> Option<Gc<crate::runtime::NativeClosure>> {
        self.running_natives.last().map(|a| a.nc)
    }

    /// The globals of thread `co` (5.1 `LUA_GLOBALSINDEX`).
    pub(crate) fn host_thread_globals(&self, co: Gc<Coro>) -> Gc<Table> {
        if self.host_is_running(co) {
            self.globals
        } else if self.is_main_coro(co) {
            self.main_ctx.as_ref().map_or(self.globals, |m| m.globals)
        } else {
            co.globals
        }
    }

    /// Replace the globals of thread `co` (5.1 `lua_replace` into
    /// `LUA_GLOBALSINDEX`).
    pub(crate) fn host_set_thread_globals(&mut self, co: Gc<Coro>, t: Gc<Table>) {
        if self.host_is_running(co) {
            self.set_globals(t);
        } else if self.is_main_coro(co) {
            // the main thread's context is parked while a coroutine runs
            if let Some(m) = self.main_ctx.as_mut() {
                m.globals = t;
            }
        } else {
            // SAFETY: `co` is held by the caller and not running, so no
            // context is loaded from it and no other reference into it is
            // live; the borrow covers one store
            unsafe { co.as_mut() }.globals = t;
            self.heap.barrier_back(co);
        }
    }

    /// Results the call of the running native wants (-1 for all), as its
    /// caller set them; read at the native's entry.
    pub(crate) fn host_native_nresults(&self) -> i32 {
        self.native_nresults
    }

    /// Why the running thread cannot yield now, as PUC words it, or `None`.
    pub(crate) fn host_yield_refusal(&self) -> Option<&'static str> {
        self.yield_barrier()
    }

    /// PUC `lua_isyieldable` of `co` (the running thread when `None`).
    pub(crate) fn host_is_yieldable(&self, co: Option<Gc<Coro>>) -> bool {
        match co {
            Some(c) if !self.host_is_running(c) => self.is_yieldable(Some(c)),
            _ => self.is_yieldable(None),
        }
    }

    /// Whether a yield is on its way out (the `Err` a call returned is the
    /// yield, not an error).
    pub(crate) fn host_yielding(&self) -> bool {
        self.yielding.is_some()
    }

    /// Call `f(args)` from a C function (PUC `lua_callk`). With `cont` and
    /// a thread that can yield, a yield inside `f` suspends the coroutine
    /// with the continuation on the frame stack: the `Err` that comes back
    /// is then the yield (`Vm::host_yielding`), and the C function must
    /// leave. Without `cont` the call cannot yield.
    pub(crate) fn host_call(
        &mut self,
        f: Value,
        args: &[Value],
        cont: Option<HostContSpec>,
    ) -> Result<Vec<Value>, LuaError> {
        let Some(spec) = cont.filter(|_| self.yield_barrier().is_none()) else {
            return self.call_value(f, args);
        };
        let results_at = self.stack.len() as u32;
        self.push_host_cont(spec, results_at)?;
        let r = self.call_value_k(f, args);
        if r.is_err() && self.yielding.is_some() {
            return r;
        }
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        r
    }

    /// Suspend the running coroutine from the C function at `func_slot`
    /// (PUC `lua_yieldk`), yielding `vals`. With `cont`, the resume runs
    /// the continuation; without, the resume's values are the C function's
    /// results. The caller has checked `Vm::host_yield_refusal`.
    pub(crate) fn host_yield(
        &mut self,
        func_slot: u32,
        nresults: i32,
        vals: Vec<Value>,
        cont: Option<u32>,
    ) -> LuaError {
        self.note_yield_ci_top();
        match cont {
            Some(token) => {
                let at = self.stack.len() as u32;
                let spec = HostContSpec {
                    func_slot,
                    nresults,
                    token,
                };
                if let Err(e) = self.push_host_cont(spec, at) {
                    return e;
                }
                self.yielding = Some((vals, at, -1));
            }
            None => self.yielding = Some((vals, func_slot, nresults)),
        }
        LuaError(Value::Nil)
    }

    /// Resume `co` with `args` (PUC `lua_resume`). `Ok` carries what it
    /// yielded or returned (its status tells which); `Err` the error that
    /// killed it.
    pub(crate) fn host_resume(
        &mut self,
        co: Gc<Coro>,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, LuaError> {
        self.resume_coro(co, args)
    }

    /// Fire the return hook of the running C function now, while its own
    /// stack still holds its arguments, what it pushed and its `n` results,
    /// the first of them at transfer index `ftransfer` (PUC `rethook` runs
    /// before the results move); its return then skips the hook.
    pub(crate) fn host_c_return_hook(&mut self, ftransfer: u32, n: u32) -> Result<(), LuaError> {
        let armed = |vm: &Vm| {
            vm.hook.ret && !vm.in_hook && (vm.hook.func.is_some() || vm.hook.rust_func.is_some())
        };
        if !armed(self) {
            return Ok(());
        }
        self.hook_return(true, ftransfer, n)?;
        // a hook that turned itself off leaves nothing to skip
        self.native_ret_hooked = armed(self);
        Ok(())
    }

    /// Hand one piece of a warning to the C API's warning function, if it
    /// installed one: `None` when the default one is to handle it. The
    /// function may install another one while it runs (PUC's own warning
    /// functions do); it is put back only when it did not.
    pub(crate) fn host_warn_piece(
        &mut self,
        msg: &[u8],
        to_cont: bool,
    ) -> Option<Result<(), LuaError>> {
        let mut w = self.host_warn.take()?;
        let r = w(self, msg, to_cont);
        if self.host_warn.is_none() {
            self.host_warn = Some(w);
        }
        Some(r)
    }
}
