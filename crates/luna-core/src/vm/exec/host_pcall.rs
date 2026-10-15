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
        let level = level?;
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(f);
        call_args.extend_from_slice(args);
        let mut results = self.call_value_host_level(level, &call_args)?;
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
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
    fn call_value_host_level(&mut self, level: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        let main = self.current.is_none();
        self.g.host_pcalls += u32::from(main);
        let r = self.call_value(level, args);
        self.g.host_pcalls -= u32::from(main);
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
        let level = level?;
        let mut call_args = Vec::with_capacity(args.len() + 2);
        call_args.push(f);
        call_args.push(msgh);
        call_args.extend_from_slice(args);
        let mut results = self.call_value_host_level(level, &call_args)?;
        // the protected call's `true, results...` or `false, handled error`
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
            Err(LuaError(results.get(1).copied().unwrap_or(Value::Nil)))
        }
    }
}
