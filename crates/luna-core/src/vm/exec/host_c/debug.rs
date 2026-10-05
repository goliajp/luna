//! The debug interface for the C API: stack levels, function information,
//! locals, upvalues and hooks.
//!
//! Levels are those of the debug library (`callstack`), on any thread: the
//! running one, a suspended or normal coroutine, or the main thread while a
//! coroutine runs. Level 0 is the innermost.

use super::*;
use crate::vm::callstack::{Ar, DbgKind, LocalSlot, ThreadStack};
use crate::vm::lib_debug;

#[doc(hidden)]
/// The C API's runner of a C hook function: the hook (a `lua_Hook`
/// pointer), the event (`b"call"`, `b"return"`, `b"tail call"`,
/// `b"tail return"`, `b"line"`, `b"count"`) and the line of a line event.
pub type HostHookFn = fn(&mut Vm, *const (), &[u8], Option<i64>) -> Result<(), LuaError>;

/// What runs at one stack level.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum HostLevel {
    /// a Lua function
    Lua,
    /// a 5.1 lost tail call
    Tail,
    /// a C function
    C {
        /// the function
        func: Value,
        /// the Lua stack slot it was called at
        func_slot: u32,
    },
}

/// PUC `lua_Debug`'s fields, all filled.
#[doc(hidden)]
pub struct HostAr {
    /// `"Lua"`, `"C"`, `"main"` or `"tail"`
    pub what: &'static str,
    /// the chunk name
    pub source: Vec<u8>,
    /// the chunk name for messages (`luaO_chunkid`)
    pub short_src: Vec<u8>,
    /// first line of the definition
    pub linedefined: i64,
    /// last line of the definition
    pub lastlinedefined: i64,
    /// the line running; -1 without one
    pub currentline: i64,
    /// `(namewhat, name)`; `None` is PUC's `namewhat = ""`, `name = NULL`
    pub name: Option<(&'static str, String)>,
    /// called by a tail call
    pub istailcall: bool,
    /// 5.5: `__call` metamethods that reached it
    pub extraargs: i64,
    /// index of the first value a hook's call or return transfers
    pub ftransfer: i64,
    /// number of values transferred
    pub ntransfer: i64,
    /// number of upvalues
    pub nups: i64,
    /// number of parameters
    pub nparams: i64,
    /// takes varargs
    pub isvararg: bool,
    /// the function; nil for a 5.1 lost tail call
    pub func: Value,
}

impl From<Ar> for HostAr {
    fn from(a: Ar) -> HostAr {
        HostAr {
            what: a.what,
            source: a.source,
            short_src: a.short_src,
            linedefined: a.linedefined,
            lastlinedefined: a.lastlinedefined,
            currentline: a.currentline,
            name: a.name,
            istailcall: a.istailcall,
            extraargs: a.extraargs,
            ftransfer: a.ftransfer,
            ntransfer: a.ntransfer,
            nups: a.nups,
            nparams: a.nparams,
            isvararg: a.isvararg,
            func: a.func,
        }
    }
}

#[doc(hidden)]
impl Vm {
    /// Install the C API's runner of C hooks.
    pub fn set_host_hook(&mut self, f: HostHookFn) {
        self.host_hook = Some(f);
    }

    /// The call stack of thread `co`.
    fn host_levels(&self, co: Gc<Coro>) -> ThreadStack<'_> {
        if self.host_is_running(co) {
            return self.thread_stack(None);
        }
        if !self.is_main_coro(co) {
            return self.thread_stack(Some(co));
        }
        // the main thread waits on the coroutines it resumed: its natives
        // end where the first of them starts
        let m = self
            .main_ctx
            .as_ref()
            .expect("the main thread's saved context");
        let mut end = self.natives_base;
        let mut c = self.current;
        while let Some(co) = c {
            if co.resumer.is_none() {
                break;
            }
            c = co.resumer;
            if let Some(r) = c {
                end = r.natives.start;
            }
        }
        ThreadStack::new(
            self.version <= LuaVersion::Lua51,
            &m.frames,
            &m.stack,
            m.top,
            &self.running_natives[..end],
            None,
        )
    }

    /// How many levels thread `co` has.
    pub fn host_level_count(&self, co: Gc<Coro>) -> usize {
        self.host_levels(co).levels.len()
    }

    /// What runs at level `i` of `co`.
    pub fn host_level(&self, co: Gc<Coro>, i: usize) -> Option<HostLevel> {
        let ts = self.host_levels(co);
        Some(match *ts.levels.get(i)? {
            DbgKind::Lua(_) => HostLevel::Lua,
            DbgKind::Tail => HostLevel::Tail,
            DbgKind::C(_) => HostLevel::C {
                func: ts.func(i),
                func_slot: ts.func_slot(i).expect("a C level has a slot"),
            },
        })
    }

    /// PUC `lua_getinfo` for level `i` of `co`.
    pub fn host_level_info(&self, co: Gc<Coro>, i: usize) -> HostAr {
        let ts = self.host_levels(co);
        self.level_ar(&ts, i).into()
    }

    /// PUC `lua_getinfo(">")` for a function value.
    pub fn host_function_info(&self, f: Value) -> HostAr {
        self.function_ar(f).into()
    }

    /// PUC 5.1 `info_tailcall`: what `lua_getinfo` reports of a lost tail
    /// call.
    pub fn host_tail_info(&self) -> HostAr {
        crate::vm::callstack::tail_ar().into()
    }

    /// PUC `lua_getinfo`'s `L` option: the lines of `f` holding an
    /// instruction, as a set; nil for a C function.
    pub fn host_active_lines(&mut self, f: Value) -> Value {
        lib_debug::activelines(self, f)
    }

    /// PUC `lua_getlocal`: the name and value of local `n` of level `i` of
    /// `co` (negative `n`: a vararg, 5.2+).
    pub fn host_local(&self, co: Gc<Coro>, i: usize, n: i64) -> Option<(String, Value)> {
        let ts = self.host_levels(co);
        let (name, at) = self.find_local(&ts, i, n)?;
        let v = match at {
            LocalSlot::Held(v) => v,
            // luna sizes the value stack lazily; a slot past it holds nil
            LocalSlot::Stack(s) => ts.stack.get(s).copied().unwrap_or(Value::Nil),
        };
        Some((name, v))
    }

    /// PUC `lua_setlocal`: store `v` in local `n` of level `i` of `co`, and
    /// return its name.
    pub fn host_set_local(&mut self, co: Gc<Coro>, i: usize, n: i64, v: Value) -> Option<String> {
        let ts = self.host_levels(co);
        let (name, at) = self.find_local(&ts, i, n)?;
        drop(ts);
        if let LocalSlot::Stack(slot) = at {
            let stack = if self.host_is_running(co) {
                &mut self.stack
            } else if self.is_main_coro(co) {
                &mut self.main_ctx.as_mut().expect("main context").stack
            } else {
                // the coroutine's saved stack is traced through `co`
                self.heap.barrier_back(co);
                // SAFETY: `co` is a thread the caller holds and not the running
                // one, so the Vm holds no reference into its saved stack; the
                // borrow lasts until the slot is written, which does not collect
                unsafe { &mut co.as_mut().stack }
            };
            if stack.len() <= slot {
                stack.resize_or_abort(slot + 1, Value::Nil);
            }
            stack[slot] = v;
        }
        // a value PUC's C library function keeps in a slot luna's native
        // does not have cannot be written
        Some(name)
    }

    /// PUC `lua_getlocal(L, NULL, n)`: the name of parameter `n` of a Lua
    /// function.
    pub fn host_param_name(&self, f: Value, n: i64) -> Option<String> {
        match f {
            Value::Closure(cl) => lib_debug::param_name(cl, n),
            _ => None,
        }
    }

    /// The name of a C function's stack slot in `lua_getlocal`.
    pub fn host_c_temporary_name(&self) -> &'static str {
        self.temporary_locvar_name()
    }

    /// PUC `lua_getupvalue` of a Lua function: the name and value of
    /// upvalue `n`.
    pub fn host_upvalue(&self, f: Value, n: i64) -> Option<(String, Value)> {
        let Value::Closure(cl) = f else { return None };
        let idx = lib_debug::visible_upvalue_index(self, cl, n)?;
        let name = lib_debug::upvalue_name(self, cl, idx)?;
        Some((name, self.upvalue_value(cl, idx)))
    }

    /// PUC `lua_setupvalue` of a Lua function: store `v` in upvalue `n`
    /// and return its name.
    pub fn host_set_upvalue(&mut self, f: Value, n: i64, v: Value) -> Option<String> {
        let Value::Closure(cl) = f else { return None };
        let idx = lib_debug::visible_upvalue_index(self, cl, n)?;
        let name = lib_debug::upvalue_name(self, cl, idx)?;
        self.upvalue_set_value(cl, idx, v);
        Some(name)
    }

    /// PUC `lua_upvalueid` of a Lua function: the address of the cell of
    /// upvalue `n`.
    pub fn host_upvalue_id(&self, f: Value, n: i64) -> Option<*const ()> {
        let Value::Closure(cl) = f else { return None };
        let i = usize::try_from(n.checked_sub(1)?).ok()?;
        cl.upvals().get(i).map(|u| u.as_ptr() as *const ())
    }

    /// PUC `lua_upvaluejoin`: upvalue `n1` of `f1` refers to upvalue `n2`
    /// of `f2`. Both are Lua functions with those upvalues.
    pub fn host_upvalue_join(&mut self, f1: Value, n1: i64, f2: Value, n2: i64) {
        let (Value::Closure(f1), Value::Closure(f2)) = (f1, f2) else {
            return;
        };
        let (Ok(i1), Ok(i2)) = (usize::try_from(n1 - 1), usize::try_from(n2 - 1)) else {
            return;
        };
        let (Some(_), Some(&uv)) = (f1.upvals().get(i1), f2.upvals().get(i2)) else {
            return;
        };
        // SAFETY: `f1` is a closure the caller holds on its stack; `uv` is a
        // separate handle read out before the borrow, which covers one store
        unsafe { f1.as_mut() }.upvals_mut()[i1] = uv;
        self.heap.barrier_back(f1);
    }

    /// The hook state of thread `co`.
    pub fn host_hook_state(&self, co: Gc<Coro>) -> HookState {
        if self.host_is_running(co) {
            self.hook
        } else if self.is_main_coro(co) {
            self.main_ctx.as_ref().map_or(self.hook, |m| m.hook)
        } else {
            co.hook
        }
    }

    /// Install `state` as the hook of thread `co` (PUC `lua_sethook`).
    pub fn host_set_hook_state(&mut self, co: Gc<Coro>, state: HookState) {
        if self.host_is_running(co) {
            self.set_hook(None, state);
        } else if self.is_main_coro(co) {
            if let Some(m) = self.main_ctx.as_mut() {
                m.hook = state;
            }
        } else {
            self.set_hook(Some(co), state);
        }
    }

    /// The values the call or return hook running now transfers (PUC
    /// `transferinfo`): the first one's index from the function, and how
    /// many.
    pub fn host_transfer(&self) -> (i64, i64) {
        (
            i64::from(self.hook_ftransfer),
            i64::from(self.hook_ntransfer),
        )
    }

    /// The C hook running now yields (`lua_yield` in a line or count
    /// hook); the coroutine suspends once the hooks of the instruction have
    /// run.
    pub fn host_hook_yield(&mut self) {
        self.hook_yield = true;
    }

    /// Drop a hook yield nothing acts on: one asked for in a call or return
    /// hook.
    pub fn host_hook_yield_clear(&mut self) {
        self.hook_yield = false;
    }

    /// The next Lua function called is called by a hook (PUC
    /// `CIST_HOOKED` on its caller): its name reads "hook" (5.3+).
    pub fn host_mark_hook_call(&mut self, on: bool) {
        self.pending_is_hook = on;
    }

    /// The level of `co` a C function of the C API runs at, by the Lua stack
    /// slot it was called at.
    pub fn host_level_of_slot(&self, co: Gc<Coro>, func_slot: u32) -> Option<usize> {
        let ts = self.host_levels(co);
        (0..ts.levels.len())
            .find(|&i| matches!(ts.levels[i], DbgKind::C(_)) && ts.func_slot(i) == Some(func_slot))
    }
}
