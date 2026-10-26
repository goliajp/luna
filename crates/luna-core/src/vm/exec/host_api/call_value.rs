//! Calling a Lua value from the host or from a native.

use super::*;

impl Vm {
    /// Call any callable value from the host (or from natives like pcall).
    ///
    /// Like PUC's `lua_call`, the call has no continuation, so the code it
    /// runs cannot yield: a `coroutine.yield` below it fails with "attempt
    /// to yield across a C-call boundary".
    pub fn call_value(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        self.nny += 1;
        let r = self.call_value_k(f, args);
        self.nny -= 1;
        r
    }

    /// [`Vm::call_value`] from a native with `extra` values pushed where
    /// PUC's C function would have them: the callee starts above them.
    pub(crate) fn call_value_pushed(
        &mut self,
        f: Value,
        args: &[Value],
        extra: u32,
    ) -> Result<Vec<Value>, LuaError> {
        self.native_push(extra);
        let r = self.call_value(f, args)?;
        self.native_pop(extra);
        Ok(r)
    }

    /// [`Vm::call_value_pushed`] with the values the native pushes below
    /// the function, `held`, written where they go.
    pub(crate) fn call_value_holding(
        &mut self,
        f: Value,
        args: &[Value],
        held: &[Value],
    ) -> Result<Vec<Value>, LuaError> {
        self.native_hold_top(held)?;
        self.call_value_pushed(f, args, held.len() as u32)
    }

    /// [`Vm::call_value`] placed at slot `at` of the running native's
    /// stack, where a C function that moved the callee below its arguments
    /// calls it (`lua_insert` then `lua_call`).
    pub(crate) fn call_value_at(
        &mut self,
        f: Value,
        args: &[Value],
        at: u32,
    ) -> Result<Vec<Value>, LuaError> {
        self.nny += 1;
        let r = self.call_value_impl(f, args, true, Some(at));
        self.nny -= 1;
        r
    }

    /// Whether the error a call just returned is not an error but a yield,
    /// a cooperative yield or a coroutine closing itself, on its way out to
    /// where it is handled: no native may catch it.
    pub(crate) fn control_in_flight(&self) -> bool {
        self.yielding.is_some()
            || self.terminating.is_some()
            || self.host_yield_pending
            || self.pending_async_native_fut.is_some()
    }

    /// [`Vm::call_value`] for a caller that can be continued after a yield
    /// below it (PUC `lua_callk`): the yield unwinds out of this call.
    pub(crate) fn call_value_k(
        &mut self,
        f: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, LuaError> {
        // host-level entry (no enclosing exec): drop any error state from a
        // prior call that propagated uncaught (`error_traceback` would
        // otherwise leak into the next debug.traceback call).
        if self.public_call_depth == 0 {
            self.error_traceback = None;
            self.dead_snapshot = None;
        }
        self.public_call_depth += 1;
        // a compiled function is entered where the call is made
        // (`try_jit_call_op`), its level counted as any call's
        let r = self.call_value_impl(f, args, true, None);
        if let Err(e) = r
            && self.public_call_depth == 1
            && self.current.is_none()
        {
            self.raise_native_to_host(e.0);
        }
        self.public_call_depth -= 1;
        r
    }

    /// [`Vm::call_value_k`] with the callee at slot `at`.
    pub(crate) fn call_value_k_at(
        &mut self,
        f: Value,
        args: &[Value],
        at: u32,
    ) -> Result<Vec<Value>, LuaError> {
        if self.public_call_depth == 0 {
            self.error_traceback = None;
            self.dead_snapshot = None;
        }
        self.public_call_depth += 1;
        let r = self.call_value_impl(f, args, true, Some(at));
        if let Err(e) = r
            && self.public_call_depth == 1
            && self.current.is_none()
        {
            self.raise_native_to_host(e.0);
        }
        self.public_call_depth -= 1;
        r
    }

    /// `call_value` with control over the `from_c` debug boundary. A `__close`
    /// handler runs *within* the closing Lua frame's activation (PUC luaF_close
    /// invokes it inside that ci), so it is called with `from_c = false`: its
    /// debug parent is the closing function, not a synthetic C level.
    /// `at`: the slot to call at, PUC's `L->top` where a message handler
    /// runs (see `raise_top`); the slots above it belong to the frame
    /// that raised, dead past that top as PUC's are. Else the running
    /// native's top (`Vm::native_top`), or the stack's end.
    pub(crate) fn call_value_impl(
        &mut self,
        f: Value,
        args: &[Value],
        from_c: bool,
        at: Option<u32>,
    ) -> Result<Vec<Value>, LuaError> {
        // the C level, and the native stack too (a first level is no
        // nesting: unchecked); both tested here, the errors out of line
        let n = self.g.nccalls;
        if n > 0 && (n + 1 >= MAX_C_DEPTH || crate::native_stack::nested_call_low()) {
            self.callback_limits()?;
        }
        self.g.nccalls = n + 1;
        let len = self.stack.len();
        // a native calls back at its own top, as a C function does
        let callback = at.is_none();
        let at = at.or_else(|| self.native_top());
        let placed = match at {
            None => self.place_call(len as u32, f, args).inspect(|_| {
                self.top = self.stack.len() as u32;
            }),
            Some(slot) => self.place_call(slot, f, args).inspect(|&slot| {
                self.top = self.stack.len().max(slot as usize + 1 + args.len()) as u32;
            }),
        };
        let func_slot = match placed {
            Ok(slot) => slot,
            Err(e) => {
                self.g.nccalls -= 1;
                return Err(e);
            }
        };
        let r = self.call_at(func_slot, args.len() as u32, from_c);
        self.g.nccalls -= 1;
        // 5.1's `luaD_call` checks for a collection when the call returns
        if let Ok(vals) = &r
            && self.version == crate::version::LuaVersion::Lua51
        {
            self.rooted(vals, |vm| {
                vm.call_return_checkpoint(func_slot + vals.len() as u32)
            })?;
        }
        // a call placed inside a frame's window gives the window back: the
        // frames below run on when the error is caught
        if at.is_some() && self.stack.len() < len {
            self.stack.regrow(len, Value::Nil);
        }
        if r.is_err() && !self.control_in_flight() {
            self.drop_failed_call(callback, len, func_slot);
        }
        r
    }

    /// A call at `func_slot` failed with a real error: its slots go (the
    /// frames below a native that called back, at `len`, run on). A yield
    /// in flight (`coroutine.yield`'s sentinel, a host's cooperative yield)
    /// or a self-close is no such error: the suspended frames above
    /// `func_slot` survive for the next resume, or the dying thread's state
    /// is discarded whole.
    #[cold]
    fn drop_failed_call(&mut self, callback: bool, len: usize, func_slot: u32) {
        let keep = if callback {
            len.max(func_slot as usize)
        } else {
            func_slot as usize
        };
        self.stack.truncate(keep);
        self.top = func_slot;
    }

    /// A nested call at the C level limit or with less than [`RESERVE`] of
    /// the native stack left fails, unless a message handler runs on that
    /// error: it may go a few levels further, and use half the reserve.
    #[cold]
    #[inline(never)]
    fn callback_limits(&mut self) -> Result<(), LuaError> {
        if is_low(RESERVE) {
            if self.msgh_depth == 0 {
                return Err(self.runerror("C stack overflow"));
            }
            if is_low(HANDLER_RESERVE) {
                return Err(LuaError(self.errerr()));
            }
        }
        self.check_c_level(true)
    }
}
