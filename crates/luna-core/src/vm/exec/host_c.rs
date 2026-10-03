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

mod block;
mod debug;
mod libs;
mod load;
mod threads;
mod values;
pub use block::HostBlock;
pub use debug::{HostAr, HostHookFn, HostLevel};
pub use values::{HOST_OP_BNOT, HOST_OP_UNM};

/// The C API's side of a [`ContKind::Host`] continuation.
#[derive(Clone, Copy)]
pub struct HostContHooks {
    /// Run continuation `token` of `thread`, whose C function sits at
    /// `func_slot`, with `values` (the results of the call it made, or what
    /// the resume passed): write the C function's results from `func_slot`
    /// on and return how many there are. A yield or an error the
    /// continuation raises comes back as the `Err`.
    pub resume: fn(&mut Vm, Gc<Coro>, u32, u32, Vec<Value>) -> Result<u32, LuaError>,
    /// Continuation `token` of `thread` will never run: an error left its C
    /// function.
    pub discard: fn(&mut Vm, Gc<Coro>, u32),
    /// A thread the C API has seen is about to be resumed.
    pub resuming: fn(&mut Vm, Gc<Coro>),
    /// A thread the C API has seen was closed: its C frames and C stack
    /// go.
    pub reset: fn(&mut Vm, Gc<Coro>),
}

/// A warning function that replaces the default one (PUC `lua_setwarnf`):
/// called with the Vm, each piece of a warning and whether more pieces
/// follow. An error it raises leaves the call that warned.
pub type HostWarn = Box<dyn FnMut(&mut Vm, &[u8], bool) -> Result<(), LuaError>>;

/// The continuation a C function keeps while it calls a function or
/// yields: see [`Vm::host_call`].
#[derive(Clone, Copy)]
pub struct HostContSpec {
    /// stack slot of the running C function
    pub func_slot: u32,
    /// results the C function's caller wants (-1 for all)
    pub nresults: i32,
    /// the C API's index for the continuation
    pub token: u32,
}

#[doc(hidden)]
impl Vm {
    /// Install the C API's continuation hooks.
    pub fn set_host_cont_hooks(&mut self, hooks: HostContHooks) {
        self.host_cont_hooks = Some(hooks);
    }

    /// Install a warning function in place of the default one, or put the
    /// default back (`None`).
    pub fn set_host_warn(&mut self, warn: Option<HostWarn>) {
        self.host_warn = warn;
    }

    /// The thread that is running: a coroutine, or the main thread's
    /// identity object.
    pub fn host_thread(&self) -> Gc<Coro> {
        self.current
            .unwrap_or_else(|| self.main_coro.expect("main thread"))
    }

    /// The main thread's identity object.
    pub fn host_main_thread(&self) -> Gc<Coro> {
        self.main_coro.expect("main thread")
    }

    /// Whether `co` is the thread that is running.
    pub fn host_is_running(&self, co: Gc<Coro>) -> bool {
        self.host_thread().ptr_eq(co)
    }

    /// A new thread with no function yet (PUC `lua_newthread`): the first
    /// resume takes the function from the thread's C stack.
    pub fn host_new_thread(&mut self) -> Gc<Coro> {
        self.new_coro(Value::Nil)
    }

    /// Set the function a thread made by [`Vm::host_new_thread`] runs.
    pub fn host_set_body(&mut self, co: Gc<Coro>, body: Value) {
        // SAFETY: `co` is held by the caller and has not started, so no
        // context is loaded from it and no other reference into it is live;
        // the borrow covers one store
        unsafe { co.as_mut() }.body = body;
        self.heap.barrier_back(co);
    }

    /// A value's text without metamethods, as `lua_tolstring` converts a
    /// number (`tostring` of other values).
    pub fn host_basic_text(&mut self, v: Value) -> Vec<u8> {
        self.tostring_basic(v)
    }

    /// The number a string converts to, as the dialect converts it (PUC
    /// `cvt2num` / `lua_stringtonumber`).
    pub fn host_str_to_number(&self, s: &[u8]) -> Option<Value> {
        Some(match str_to_num(s, self.version)? {
            numeric::Num::Int(i) => Value::Int(i),
            numeric::Num::Float(f) => Value::Float(f),
        })
    }

    /// The function the C API made for the light C function `key`, made
    /// with `make` the first time (5.2+'s functions without upvalues, which
    /// PUC compares by their C pointer). It lives as long as the Vm.
    pub fn host_light_fn(&mut self, key: usize, make: impl FnOnce(&mut Vm) -> Value) -> Value {
        if let Some(&v) = self.host_light.get(&key) {
            return v;
        }
        let v = make(self);
        self.host_light.insert(key, v);
        v
    }

    /// The native running on top of the dispatch chain.
    pub fn host_running_native(&self) -> Option<Gc<crate::runtime::NativeClosure>> {
        self.running_natives.last().map(|a| a.nc)
    }

    /// The globals of thread `co` (5.1 `LUA_GLOBALSINDEX`).
    pub fn host_thread_globals(&self, co: Gc<Coro>) -> Gc<Table> {
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
    pub fn host_set_thread_globals(&mut self, co: Gc<Coro>, t: Gc<Table>) {
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
    pub fn host_native_nresults(&self) -> i32 {
        self.native_nresults
    }

    /// Why the running thread cannot yield now, as PUC words it, or `None`.
    pub fn host_yield_refusal(&self) -> Option<&'static str> {
        self.yield_barrier()
    }

    /// PUC `lua_isyieldable` of `co` (the running thread when `None`).
    pub fn host_is_yieldable(&self, co: Option<Gc<Coro>>) -> bool {
        match co {
            Some(c) if !self.host_is_running(c) => self.is_yieldable(Some(c)),
            _ => self.is_yieldable(None),
        }
    }

    /// Whether a yield is on its way out (the `Err` a call returned is the
    /// yield, not an error).
    pub fn host_yielding(&self) -> bool {
        self.yielding.is_some()
    }

    /// Call `f(args)` from a C function (PUC `lua_callk`). With `cont` and
    /// a thread that can yield, a yield inside `f` suspends the coroutine
    /// with the continuation on the frame stack: the `Err` that comes back
    /// is then the yield ([`Vm::host_yielding`]), and the C function must
    /// leave. Without `cont` the call cannot yield.
    pub fn host_call(
        &mut self,
        f: Value,
        args: &[Value],
        cont: Option<HostContSpec>,
    ) -> Result<Vec<Value>, LuaError> {
        let Some(spec) = cont.filter(|_| self.yield_barrier().is_none()) else {
            return self.call_noyield(f, args);
        };
        let results_at = self.stack.len() as u32;
        self.push_host_cont(spec, results_at);
        let r = self.call_value(f, args);
        if r.is_err() && self.yielding.is_some() {
            return r;
        }
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        r
    }

    /// Suspend the running coroutine from the C function at `func_slot`
    /// (PUC `lua_yieldk`), yielding `vals`. With `cont`, the resume runs
    /// the continuation; without, the resume's values are the C function's
    /// results. The caller has checked [`Vm::host_yield_refusal`]. Returns
    /// the error value the native returns to leave.
    pub fn host_yield(
        &mut self,
        func_slot: u32,
        nresults: i32,
        vals: Vec<Value>,
        cont: Option<u32>,
    ) -> LuaError {
        match cont {
            Some(token) => {
                let at = self.stack.len() as u32;
                let spec = HostContSpec {
                    func_slot,
                    nresults,
                    token,
                };
                self.push_host_cont(spec, at);
                self.yielding = Some((vals, at, -1));
            }
            None => self.yielding = Some((vals, func_slot, nresults)),
        }
        LuaError(Value::Nil)
    }

    /// Resume `co` with `args` (PUC `lua_resume`). `Ok` carries what it
    /// yielded or returned (its status tells which); `Err` the error that
    /// killed it.
    pub fn host_resume(&mut self, co: Gc<Coro>, args: Vec<Value>) -> Result<Vec<Value>, LuaError> {
        self.resume_coro(co, args)
    }

    /// The function a C API protected call (`lua_pcallk`) calls, which is
    /// not a level of the stack: with a handler it takes `(f, msgh,
    /// args...)`, without `(f, args...)`; it returns `true, results...` or
    /// `false, error`.
    pub fn host_protected_fn(&mut self, handler: bool) -> Value {
        let f: crate::runtime::value::NativeFn = if handler {
            crate::vm::builtins::nat_host_xpcall
        } else {
            crate::vm::builtins::nat_host_pcall
        };
        self.native(f)
    }

    /// How many errors have taken a status of their own: "error in error
    /// handling" (PUC's `LUA_ERRERR`), and 5.2/5.3's finalizer error of a
    /// full collection (`LUA_ERRGCMM`). A protected call compares it before
    /// and after, and tells them apart by the message.
    pub fn host_errerr_count(&self) -> u64 {
        self.errerr_raised + self.gcmm_raised
    }

    /// Close `co` (PUC `lua_closethread`): run its pending `__close`
    /// handlers and mark it dead. `Ok(Some(e))`: it had died with `e`.
    pub fn host_close_thread(&mut self, co: Gc<Coro>) -> Result<Option<Value>, LuaError> {
        self.close_coro(co)
    }

    fn push_host_cont(&mut self, spec: HostContSpec, results_at: u32) {
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Host(HostCont {
                    results_at,
                    token: spec.token,
                }),
                func_slot: spec.func_slot,
                nresults: spec.nresults,
            }),
        );
    }

    /// A [`ContKind::Host`] frame came to the top (already popped): its call
    /// returned, or its coroutine was resumed. Run the continuation with the
    /// C function back among the running natives, and finish the C
    /// function's call with what the continuation returns.
    pub(crate) fn finish_host_cont(
        &mut self,
        nc: NativeCont,
        hc: HostCont,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let at = hc.results_at as usize;
        let top = (self.top as usize).max(at);
        let vals = self.stack[at..top].to_vec();
        self.stack.truncate(at);
        self.top = hc.results_at;
        let hooks = self
            .host_cont_hooks
            .expect("a C continuation without the C API");
        let Value::Native(ncl) = self.stack[nc.func_slot as usize] else {
            unreachable!("a C continuation's slot holds its C function")
        };
        self.running_natives.push(NativeAct {
            nc: ncl,
            func_slot: nc.func_slot,
            nargs: 0,
            depth: self.frames.len() as u32,
            ccmt: 0,
        });
        self.native_nresults = nc.nresults;
        let thread = self.host_thread();
        match (hooks.resume)(self, thread, hc.token, nc.func_slot, vals) {
            Ok(nret) => {
                if self.frames.len() < entry_depth {
                    self.running_natives.pop();
                    self.top = nc.func_slot + nret;
                    return Ok(Some(self.take_results(nc.func_slot)));
                }
                self.finish_native_call(nc.func_slot, 0, nret, nc.nresults)?;
                Ok(None)
            }
            Err(e) => {
                let act = self.running_natives.pop().expect("pushed above");
                if self.yielding.is_none() {
                    self.note_errored_native(act, e.0);
                }
                Err(e)
            }
        }
    }

    /// An error dropped a [`ContKind::Host`] frame.
    pub(crate) fn discard_host_cont(&mut self, hc: HostCont) {
        if let Some(hooks) = self.host_cont_hooks {
            let thread = self.host_thread();
            (hooks.discard)(self, thread, hc.token);
        }
    }

    /// PUC `lua_close` before the state is freed: close the main thread's
    /// pending to-be-closed variables and run every finalizer, dropping
    /// their errors.
    pub fn host_close_state(&mut self) {
        self.close_state();
    }

    /// PUC's registry (`LUA_REGISTRYINDEX`), made on first use with the
    /// entries `lua_newstate` puts in it: from 5.2 on the main thread and
    /// the globals (5.5 also `false` at index 1, which `luaL_ref` uses).
    pub fn host_registry(&mut self) -> Gc<Table> {
        if let Some(r) = self.registry {
            return r;
        }
        let reg = self.heap.new_table();
        self.registry = Some(reg);
        let main = Value::Coro(self.host_main_thread());
        let globals = Value::Table(self.globals);
        let entries: &[(i64, Value)] = match self.version {
            LuaVersion::Lua51 => &[],
            LuaVersion::Lua55 => &[(1, Value::Bool(false)), (3, main), (2, globals)],
            _ => &[(1, main), (2, globals)],
        };
        for &(k, v) in entries {
            // SAFETY: `reg` was allocated above and is rooted through
            // `self.registry`; the borrow covers one store, which does not
            // collect
            let r = unsafe { reg.as_mut() }.set_int_raw(&mut self.heap, k, v);
            debug_assert!(r.is_ok(), "a small integer key");
        }
        self.heap.barrier_back(reg);
        reg
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
