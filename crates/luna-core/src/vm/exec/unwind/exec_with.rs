//! Running the dispatcher from an entry depth until it returns, yields or fails.

use super::*;

impl Vm {
    /// Run from the current top frame down to (but not past) `entry_depth`
    /// frames. Coroutine driving passes `entry_depth = 1` so the whole thread
    /// runs to completion or a yield.
    /// Resume the dispatcher from the saved
    /// `entry_depth` (captured pre-yield by `drive_one`). Called by
    /// `EvalFuture::poll` on every poll after the first to walk the
    /// existing call frames until the next `BudgetExhausted` or
    /// terminal `Ok`/`Err`. Not a public-API surface; the
    /// embedder reaches it through `Vm::eval_async`.
    pub(crate) fn exec_with_async(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        self.exec_with(entry_depth)
    }

    pub(crate) fn exec_with(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        loop {
            let r = self.run(entry_depth);
            if r.is_err()
                && (self.yielding.is_some()
                    || self.terminating.is_some()
                    || self.host_yield_pending
                    || self.pending_async_native_fut.is_some())
            {
                // a `coroutine.yield` is in flight: keep the frames intact (they
                // are the suspended coroutine's saved state) and propagate to
                // resume. A self-close termination propagates the same way, so a
                // protecting pcall on the way out cannot catch (unwind) it.
                // `host_yield_pending` is the async-mode
                // analogue: the sentinel must reach `drive_one` without
                // a protecting `pcall` swallowing it.
                return r;
            }
            match r {
                Ok(vals) => return Ok(vals),
                // unwind toward `entry_depth`. A protecting pcall/xpcall
                // continuation caught along the way turns the error into
                // `false, msg` and the loop resumes running its caller; an
                // uncaught error propagates out.
                Err(e) => match self.unwind(e.0, entry_depth) {
                    Unwound::Caught => continue,
                    Unwound::CaughtReturn(vals) => return Ok(vals),
                    Unwound::Propagated(err) => return Err(err),
                },
            }
        }
    }
}
