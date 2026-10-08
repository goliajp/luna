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
        }
        self.public_call_depth += 1;
        // JIT fast path. A host call with no args targeting a Lua
        // chunk whose body fits the int-arith whitelist short-circuits
        // the whole interpreter dispatch and runs straight through the
        // mmap'd native code. The lookup is one Cell::get + one match —
        // the slow path (compile attempt on first reach) is paid once per
        // Proto.
        let r = match f {
            Value::Closure(cl) if args.is_empty() => self.try_jit_call(cl),
            _ => None,
        };
        let r = match r {
            Some(Ok(vs)) => {
                self.public_call_depth -= 1;
                return Ok(vs);
            }
            Some(Err(e)) => Err(e),
            None => self.call_value_impl(f, args, true, None),
        };
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
        // the native stack too (a first level is no nesting: unchecked);
        // a message handler running on the error gets half the reserve
        if self.g.nccalls > 0 && is_low(RESERVE) {
            if self.msgh_depth == 0 {
                return Err(self.runerror("C stack overflow"));
            }
            if is_low(HANDLER_RESERVE) {
                return Err(LuaError(self.errerr()));
            }
        }
        self.check_c_level(true)?;
        self.g.nccalls += 1;
        let len = self.stack.len();
        // a native calls back at its own top, as a C function does
        let callback = at.is_none();
        let at = at.or_else(|| self.native_top());
        let func_slot = match at {
            None => {
                self.stack.push_or_abort(f);
                self.stack.extend_from_slice_or_abort(args);
                self.top = self.stack.len() as u32;
                len as u32
            }
            Some(slot) => {
                self.place_call(slot, f, args);
                self.top = self.stack.len().max(slot as usize + 1 + args.len()) as u32;
                slot
            }
        };
        let r = self.call_at(func_slot, args.len() as u32, from_c);
        self.g.nccalls -= 1;
        // a call placed inside a frame's window gives the window back: the
        // frames below run on when the error is caught
        if at.is_some() && self.stack.len() < len {
            self.grow_stack_or_abort(len);
        }
        if r.is_err() && !self.control_in_flight() {
            // A `coroutine.yield` in flight raises a sentinel error to unwind the
            // Rust stack, but the suspended coroutine's frames/registers (which
            // sit at/above `func_slot`) must survive for the next resume — so we
            // only truncate on a real error. A self-close termination is in the
            // same boat: the dying thread's state is discarded wholesale.
            // A `host_yield_pending` cooperative yield is in
            // the same boat as `yielding`: the next `EvalFuture::poll`
            // resumes the same call, so the in-flight frames must
            // survive.
            // the frames below a native that called back run on
            let keep = if callback {
                len.max(func_slot as usize)
            } else {
                func_slot as usize
            };
            self.stack.truncate(keep);
            self.top = func_slot;
        }
        r
    }
}
