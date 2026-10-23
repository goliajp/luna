//! The C API's view of a Vm: the debug interface: levels, locals, upvalues, hooks.

use crate::runtime::*;
use crate::vm::exec::host_c::*;
use crate::vm::exec::*;

/// What the C API (`luna-jit`'s `capi`) asks of a Vm: the debug interface: levels, locals, upvalues, hooks.
pub trait VmCApiDebug {
    /// Install the C API's runner of C hooks.
    fn set_host_hook(&mut self, f: HostHookFn);
    /// How many levels thread `co` has.
    fn host_level_count(&self, co: Gc<Coro>) -> usize;
    /// What runs at level `i` of `co`.
    fn host_level(&self, co: Gc<Coro>, i: usize) -> Option<HostLevel>;
    /// PUC `lua_getinfo` for level `i` of `co`.
    fn host_level_info(&self, co: Gc<Coro>, i: usize) -> HostAr;
    /// PUC `lua_getinfo(">")` for a function value.
    fn host_function_info(&self, f: Value) -> HostAr;
    /// PUC 5.1 `info_tailcall`: what `lua_getinfo` reports of a lost tail
    /// call.
    fn host_tail_info(&self) -> HostAr;
    /// PUC `lua_getinfo`'s `L` option: the lines of `f` holding an
    /// instruction, as a set; nil for a C function.
    fn host_active_lines(&mut self, f: Value) -> Value;
    /// PUC `lua_getlocal`: the name and value of local `n` of level `i` of
    /// `co` (negative `n`: a vararg, 5.2+).
    ///
    /// It takes `&mut self` because a slot under a `load` reader may hold
    /// an object PUC's parser or undumper has there, which luna makes when
    /// it is first looked at.
    fn host_local(&mut self, co: Gc<Coro>, i: usize, n: i64) -> Option<(String, Value)>;
    /// PUC `lua_setlocal`: store `v` in local `n` of level `i` of `co`, and
    /// return its name.
    fn host_set_local(&mut self, co: Gc<Coro>, i: usize, n: i64, v: Value) -> Option<String>;
    /// PUC `lua_getlocal(L, NULL, n)`: the name of parameter `n` of a Lua
    /// function.
    fn host_param_name(&self, f: Value, n: i64) -> Option<String>;
    /// The name of a C function's stack slot in `lua_getlocal`.
    fn host_c_temporary_name(&self) -> &'static str;
    /// PUC `lua_getupvalue` of a Lua function: the name and value of
    /// upvalue `n`.
    fn host_upvalue(&self, f: Value, n: i64) -> Option<(String, Value)>;
    /// PUC `lua_setupvalue` of a Lua function: store `v` in upvalue `n`
    /// and return its name.
    fn host_set_upvalue(&mut self, f: Value, n: i64, v: Value) -> Option<String>;
    /// PUC `lua_upvalueid` of a Lua function: the address of the cell of
    /// upvalue `n`.
    fn host_upvalue_id(&self, f: Value, n: i64) -> Option<*const ()>;
    /// PUC `lua_upvaluejoin`: upvalue `n1` of `f1` refers to upvalue `n2`
    /// of `f2`. Both are Lua functions with those upvalues.
    fn host_upvalue_join(&mut self, f1: Value, n1: i64, f2: Value, n2: i64);
    /// The hook state of thread `co`.
    fn host_hook_state(&self, co: Gc<Coro>) -> HookState;
    /// Install `state` as the hook of thread `co` (PUC `lua_sethook`).
    fn host_set_hook_state(&mut self, co: Gc<Coro>, state: HookState);
    /// The values the call or return hook running now transfers (PUC
    /// `transferinfo`): the first one's index from the function, and how
    /// many.
    fn host_transfer(&self) -> (i64, i64);
    /// The C hook running now yields (`lua_yield` in a line or count
    /// hook); the coroutine suspends once the hooks of the instruction have
    /// run.
    fn host_hook_yield(&mut self);
    /// The next Lua function called is called by a hook (PUC
    /// `CIST_HOOKED` on its caller): its name reads "hook" (5.3+).
    fn host_mark_hook_call(&mut self, on: bool);
}

impl VmCApiDebug for crate::vm::Vm {
    #[inline]
    fn set_host_hook(&mut self, f: HostHookFn) {
        crate::vm::Vm::set_host_hook(self, f)
    }
    #[inline]
    fn host_level_count(&self, co: Gc<Coro>) -> usize {
        crate::vm::Vm::host_level_count(self, co)
    }
    #[inline]
    fn host_level(&self, co: Gc<Coro>, i: usize) -> Option<HostLevel> {
        crate::vm::Vm::host_level(self, co, i)
    }
    #[inline]
    fn host_level_info(&self, co: Gc<Coro>, i: usize) -> HostAr {
        crate::vm::Vm::host_level_info(self, co, i)
    }
    #[inline]
    fn host_function_info(&self, f: Value) -> HostAr {
        crate::vm::Vm::host_function_info(self, f)
    }
    #[inline]
    fn host_tail_info(&self) -> HostAr {
        crate::vm::Vm::host_tail_info(self)
    }
    #[inline]
    fn host_active_lines(&mut self, f: Value) -> Value {
        crate::vm::Vm::host_active_lines(self, f)
    }
    #[inline]
    fn host_local(&mut self, co: Gc<Coro>, i: usize, n: i64) -> Option<(String, Value)> {
        crate::vm::Vm::host_local(self, co, i, n)
    }
    #[inline]
    fn host_set_local(&mut self, co: Gc<Coro>, i: usize, n: i64, v: Value) -> Option<String> {
        crate::vm::Vm::host_set_local(self, co, i, n, v)
    }
    #[inline]
    fn host_param_name(&self, f: Value, n: i64) -> Option<String> {
        crate::vm::Vm::host_param_name(self, f, n)
    }
    #[inline]
    fn host_c_temporary_name(&self) -> &'static str {
        crate::vm::Vm::host_c_temporary_name(self)
    }
    #[inline]
    fn host_upvalue(&self, f: Value, n: i64) -> Option<(String, Value)> {
        crate::vm::Vm::host_upvalue(self, f, n)
    }
    #[inline]
    fn host_set_upvalue(&mut self, f: Value, n: i64, v: Value) -> Option<String> {
        crate::vm::Vm::host_set_upvalue(self, f, n, v)
    }
    #[inline]
    fn host_upvalue_id(&self, f: Value, n: i64) -> Option<*const ()> {
        crate::vm::Vm::host_upvalue_id(self, f, n)
    }
    #[inline]
    fn host_upvalue_join(&mut self, f1: Value, n1: i64, f2: Value, n2: i64) {
        crate::vm::Vm::host_upvalue_join(self, f1, n1, f2, n2)
    }
    #[inline]
    fn host_hook_state(&self, co: Gc<Coro>) -> HookState {
        crate::vm::Vm::host_hook_state(self, co)
    }
    #[inline]
    fn host_set_hook_state(&mut self, co: Gc<Coro>, state: HookState) {
        crate::vm::Vm::host_set_hook_state(self, co, state)
    }
    #[inline]
    fn host_transfer(&self) -> (i64, i64) {
        crate::vm::Vm::host_transfer(self)
    }
    #[inline]
    fn host_hook_yield(&mut self) {
        crate::vm::Vm::host_hook_yield(self)
    }
    #[inline]
    fn host_mark_hook_call(&mut self, on: bool) {
        crate::vm::Vm::host_mark_hook_call(self, on)
    }
}
