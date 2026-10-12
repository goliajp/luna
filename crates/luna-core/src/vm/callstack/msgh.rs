//! Running an xpcall message handler on an error (PUC `luaG_errormsg`).

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

impl Vm {
    /// The slot 5.1 and 5.2 `luaG_errormsg` take for the message handler
    /// it puts at `at` (`incr_top`), which may find the stack full.
    pub(super) fn errormsg_room(
        &mut self,
        at: Option<u32>,
    ) -> Result<(), crate::vm::error::LuaError> {
        match (self.version(), at) {
            (LuaVersion::Lua51, Some(at)) => self.check_lua_stack(at + 1, 1, false),
            (LuaVersion::Lua52, Some(at)) => self.check_lua_stack(at + 2, 0, false),
            _ => Ok(()),
        }
    }

    /// Run an xpcall message handler on `err`, as PUC's `luaG_errormsg`
    /// does: with the handler still installed, so an error the handler
    /// raises runs it again at that point (nested, the raising frames still
    /// on the stack), and what that inner run returns is the error thrown
    /// out of the outer one. The handler's calls take C levels like any
    /// other: past `MAX_C_DEPTH` the handler is run on the "C stack
    /// overflow" that refusing one raises, and at `errerr_c_depth` the
    /// refusal is "error in error handling", which no handler runs on
    /// (errors.lua :637).
    pub(crate) fn call_msgh(&mut self, handler: Value, err: Value) -> Value {
        self.call_msgh_at(handler, err, None)
    }

    /// [`Vm::call_msgh`] with the handler called at stack slot `at` (PUC
    /// `luaG_errormsg`, see `raise_top`).
    pub(crate) fn call_msgh_at(&mut self, handler: Value, err: Value, at: Option<u32>) -> Value {
        // 5.4+ `lua_error` raises the memory error message itself as a
        // memory error, which no handler runs on
        if self.version() >= LuaVersion::Lua54
            && let Value::Str(s) = err
            && s.as_bytes() == b"not enough memory"
        {
            self.heap.mem_ctx().raise_oom();
            return err;
        }
        // ≤5.2 `luaG_errormsg` raises LUA_ERRERR at once when the handler
        // is not a function
        if self.version() <= LuaVersion::Lua52
            && !matches!(handler, Value::Closure(_) | Value::Native(_))
        {
            return self.errerr();
        }
        self.msgh_runs += 1;
        let runs = self.msgh_runs;
        let errerrs = self.errerr_raised;
        // a call refused at the C-level limit keeps its level while its
        // handler runs (PUC's `luaD_call` never takes it back)
        let held = self.c_overflow_err.take().is_some_and(|v| v.raw_eq(err));
        self.g.nccalls += u32::from(held);
        self.msgh_depth += 1;
        let r = self.call_protected_with(handler, &[err], Some(handler), at);
        self.msgh_depth -= 1;
        self.g.nccalls -= u32::from(held);
        match r {
            Ok(results) => results.first().copied().unwrap_or(Value::Nil),
            // the handler's own call was refused with LUA_ERRERR
            Err(e) if self.msgh_runs == runs && self.errerr_raised != errerrs => e.0,
            // already the result of the handler run nested at that error
            Err(e) if self.msgh_runs != runs => e.0,
            // raised with no Lua frame to unwind (a native handler such as
            // `error` failing at once, or the call refused at the C-level
            // limit or for want of stack): no nested run saw it, so run the
            // handler on it here, one level deeper, where the refused call's
            // error was raised when that was the stack's end
            Err(e) => {
                let at = self.overflow_top.take();
                self.call_msgh_at(handler, e.0, at)
            }
        }
    }
}
