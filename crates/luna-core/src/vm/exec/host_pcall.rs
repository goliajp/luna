//! The host's protected calls: PUC `lua_pcall` made by the embedder or the
//! C API, through a native that is the protected-call level.

use super::*;
use crate::runtime::Builtin;
use crate::vm::builtins;

impl Vm {
    /// Call `f` with `args` in protected mode with the message handler
    /// `msgh`: PUC `lua_pcall(L, nargs, LUA_MULTRET, msgh)` made by the host.
    ///
    /// `msgh` runs where the error was raised, before the stack unwinds, so
    /// it can take a traceback of the failing call ([`Vm::traceback`]); an
    /// error inside it calls it again with the new error, as in PUC. The
    /// returned error carries what the handler returned.
    ///
    /// Like `lua_pcall`, the call is not a level of the stack: a traceback
    /// taken inside ends with `f`.
    pub fn call_value_with_handler(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        self.host_pcall(
            builtins::nat_host_xpcall,
            Builtin::HostXpcall,
            f,
            args,
            msgh,
        )
    }

    /// [`Vm::call_value_with_handler`] made from inside a C function of the
    /// host's, as lua.c's `docall` runs inside `pmain`: that function is one
    /// C level below `f`, which `debug.getinfo` finds and a traceback ends
    /// with (`[C]: in ?`, 5.1 `[C]: ?`).
    #[doc(hidden)]
    pub fn call_value_with_handler_in_c(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        self.host_pcall(
            builtins::nat_host_xpcall_in_c,
            Builtin::HostXpcallInC,
            f,
            args,
            msgh,
        )
    }

    /// `f(args)` in protected mode with no message handler, made from inside
    /// a C function of the host's: PUC `lua_pcall(L, n, r, 0)` inside a C
    /// function, as lua.c's `l_print` calls `print` from `pmain`. That
    /// function is one C level below `f`, as for
    /// [`Vm::call_value_with_handler_in_c`].
    #[doc(hidden)]
    pub fn call_value_in_c(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        self.heap.puc.silent += 1;
        let level = self.builtin(builtins::nat_host_pcall_in_c, &[], Builtin::HostPcallInC);
        self.heap.puc.silent -= 1;
        let level = luna_only(level?);
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(f);
        call_args.extend_from_slice(args);
        let mut results = self.call_value_host_level(level, &call_args)?;
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
            // from 5.2 `luaD_pcall` shrinks the stack once the error is set
            if self.version() >= LuaVersion::Lua52 {
                self.shrink_caught_error();
            }
            Err(LuaError(results.get(1).copied().unwrap_or(Value::Nil)))
        }
    }

    /// [`Vm::call_value_with_handler`] that also says how it failed: `true`
    /// when the handler itself failed and the error is "error in error
    /// handling" (PUC's LUA_ERRERR), `false` for any other error
    /// (LUA_ERRRUN). For the C API's `lua_pcall`.
    #[doc(hidden)]
    pub fn call_value_with_handler_status(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, (LuaError, bool)> {
        let before = self.errerr_raised;
        self.call_value_with_handler(f, args, msgh).map_err(|e| {
            // a handler may return the same text itself; only an error the
            // vm turned into it during this call is LUA_ERRERR
            let errerr = self.errerr_raised != before
                && matches!(e.0, Value::Str(s) if s.as_bytes() == b"error in error handling");
            (e, errerr)
        })
    }

    /// Call `level`, the native standing in for a protected call PUC's
    /// host makes from C, which takes no level of PUC's (on the main
    /// thread, the only one a host's call runs on).
    fn call_value_host_level(
        &mut self,
        level: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, LuaError> {
        let main = self.current.is_none();
        self.g.host_pcalls += u32::from(main);
        let r = self.call_value(level, args);
        self.g.host_pcalls -= u32::from(main);
        self.g.ci_hint = 0;
        r
    }

    /// [`Vm::call_value_host_level`] with the level at slot `at`.
    fn call_value_host_level_at(
        &mut self,
        level: Value,
        args: &[Value],
        at: u32,
    ) -> Result<Vec<Value>, LuaError> {
        self.g.host_pcalls += 1;
        self.nny += 1;
        let r = self.call_value_k_at(level, args, at);
        self.nny -= 1;
        self.g.host_pcalls -= 1;
        self.g.ci_hint = 0;
        r
    }

    fn host_pcall(
        &mut self,
        level: crate::runtime::value::NativeFn,
        builtin: Builtin,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        // the protected call PUC makes from C is no function of its own:
        // luna's stand-in for it is not counted
        self.heap.puc.silent += 1;
        let level = self.builtin(level, &[], builtin);
        self.heap.puc.silent -= 1;
        let level = luna_only(level?);
        let k = self.g.host_slots as usize;
        let docall = builtin == Builtin::HostXpcallInC
            && k > 0
            && self.current.is_none()
            && self.frames_top == 0
            && self.top as usize == k;
        let mut results = if docall {
            // lua.c's `docall`: the native takes `pmain`'s place, its
            // arguments above it, then the handler below the function
            let saved: Vec<Value> = self.stack[..k].to_vec();
            let mut call_args = Vec::with_capacity(k + 1 + args.len());
            call_args.extend_from_slice(&saved[1..]);
            call_args.push(msgh);
            call_args.push(f);
            call_args.extend_from_slice(args);
            let r = self.call_value_host_level_at(level, &call_args, 0);
            self.stack[..k].copy_from_slice(&saved);
            self.top = k as u32;
            r?
        } else {
            let mut call_args = Vec::with_capacity(args.len() + 2);
            call_args.push(f);
            call_args.push(msgh);
            call_args.extend_from_slice(args);
            self.call_value_host_level(level, &call_args)?
        };
        // the protected call's `true, results...` or `false, handled error`
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
            // from 5.2 `luaD_pcall` shrinks the stack once the error is set
            if self.version() >= LuaVersion::Lua52 {
                self.shrink_caught_error();
            }
            Err(LuaError(results.get(1).copied().unwrap_or(Value::Nil)))
        }
    }

    /// lua.c's `docall` from `pmain`, whose place the native at the bottom
    /// slot takes: the host's arguments, the handler, then the function
    /// and its own arguments, called where they are.
    pub(super) fn begin_docall(
        &mut self,
        nargs: u32,
        nresults: i32,
        level: bool,
    ) -> Result<bool, LuaError> {
        let k = self.g.host_slots;
        let callee = k + 1;
        let nfargs = nargs - k - 1;
        self.pcall_room(callee, nfargs + 1, 0)?;
        self.enter_pcall_level(level)?;
        let handler = self.stack[k as usize];
        self.top = callee + 1 + nfargs;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Xpcall {
                    handler,
                    level,
                    shift: k as i8,
                    ci_top: 0,
                },
                func_slot: 0,
                nresults,
            }),
        );
        self.begin_call(callee, Some(nfargs), -1, true)?;
        Ok(true)
    }

    /// Stack room for the callee and its `n - 1` arguments moved by
    /// `shift`, and frame room for the continuation, collecting in an
    /// emergency when memory runs out.
    pub(super) fn pcall_room(&mut self, from: u32, n: u32, shift: i8) -> Result<(), LuaError> {
        let to = (i64::from(from) + i64::from(shift)) as u32;
        self.grow_stack((to + n) as usize)?;
        self.cf_room()
    }

    /// The continuation of a protected call takes a C level when it is one
    /// of PUC's (see `begin_pcall`); the error is raised inside pcall, a C
    /// function: no position.
    pub(super) fn enter_pcall_level(&mut self, level: bool) -> Result<(), LuaError> {
        if level {
            self.check_c_level(false)?;
            self.g.nccalls += 1;
        }
        Ok(())
    }

    /// [`Vm::enter_pcall_level`] for a protected call whose own call may be
    /// refused at the C level limit: PUC's `luaD_pcall` is in place by then
    /// and catches the refusal, so the level is taken anyway, for the
    /// continuation pushed next to give back; the error is the call's.
    pub(super) fn refusable_level(&mut self, level: bool) -> Result<(), LuaError> {
        let entered = self.enter_pcall_level(level);
        if entered.is_err() && level {
            self.g.nccalls += 1;
            // its handler runs above the level the continuation holds
            self.c_overflow_err = None;
        }
        entered
    }
}

/// `f`, a native of luna's own: no function of PUC's.
fn luna_only(f: Value) -> Value {
    if let Value::Native(n) = f {
        // SAFETY: `n` is a live native; only its class byte is written
        unsafe { (*n.header()).cls |= crate::runtime::heap::LUNA_ONLY };
    }
    f
}
