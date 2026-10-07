//! Threads for the C API beyond resume and yield: status, closing, and
//! threads a host has used, which PUC treats by what is on their stack.
//!
//! In PUC a thread that has not started, or that returned, sits at its base
//! level: the next resume calls the value on top of its stack (below the
//! arguments), and it counts as dead while its stack is empty. A coroutine
//! the C API has seen (it has a `host_state`) follows that rule; one only
//! Lua has seen keeps its body in `Coro::body`, as before.

use super::*;

#[doc(hidden)]
impl Vm {
    /// Whether `co` sits at its base level with its next body on its C
    /// stack: a thread the C API has seen that has not started, returned,
    /// or was closed.
    pub fn host_restartable(&self, co: Gc<Coro>) -> bool {
        co.host_state.is_some()
            && !self.is_main_coro(co)
            && (!co.started || (co.status == CoroStatus::Dead && co.error_value.is_none()))
    }

    /// The status of `co` as `coroutine.status` reports it.
    pub fn host_coro_status(&self, co: Gc<Coro>) -> CoroStatus {
        self.effective_coro_status(co)
    }

    /// Whether a resume now would overflow PUC's C stack
    /// (`LUAI_MAXCCALLS`).
    pub fn host_c_stack_full(&self) -> bool {
        self.g.nccalls >= MAX_C_DEPTH
    }

    /// Whether the running coroutine is closing itself (the `Err` a call
    /// returned is that, not an error).
    pub fn host_terminating(&self) -> bool {
        self.terminating.is_some()
    }

    /// Close the running coroutine from inside it (PUC 5.5
    /// `lua_closethread(L, L)`): run its pending `__close` handlers; the
    /// returned value is what the C function leaves with, and the resume
    /// that runs the coroutine then ends it.
    pub fn host_close_running(&mut self) -> LuaError {
        self.close_running()
    }

    /// Before `co` is resumed (from Lua or from C): let the C API drop what
    /// it left on the thread's C stack at the last suspension, and give a
    /// thread at its base level the body on top of its C stack.
    pub(crate) fn host_before_resume(&mut self, co: Gc<Coro>) {
        if co.host_state.is_none() {
            return;
        }
        if let Some(h) = self.host_cont_hooks {
            (h.resuming)(self, co);
        }
        if !self.host_restartable(co) {
            return;
        }
        // SAFETY: `co` is held by the caller and not running, so no context
        // is loaded from it and no other reference into it is live
        let m = unsafe { co.as_mut() };
        m.body = m.host_stack.pop().unwrap_or(Value::Nil);
        m.started = false;
        m.status = CoroStatus::Suspended;
        m.resume_at = None;
        m.error_traceback = None;
        m.error_levels = None;
        m.stack.take();
        m.frames.take();
        m.open_upvals.take();
        m.tbc.take();
        m.top = 0;
        m.meta_conts = 0;
        m.stale_frames = 0;
        self.heap.barrier_back(co);
    }

    /// `co` was closed or closed itself: the C API resets what it keeps for
    /// the thread, its C stack included (PUC `luaE_resetthread`).
    pub(crate) fn host_thread_reset(&mut self, co: Gc<Coro>) {
        if co.host_state.is_none() {
            return;
        }
        if let Some(h) = self.host_cont_hooks {
            (h.reset)(self, co);
        }
    }

    /// `co` returned `outcome`: a thread the C API has seen returns what was
    /// under its body on its C stack too, as PUC's resume finds every value
    /// of the thread's stack there and `coroutine.resume` takes them all.
    pub(crate) fn host_returned(
        &mut self,
        co: Gc<Coro>,
        outcome: Result<Vec<Value>, LuaError>,
    ) -> Result<Vec<Value>, LuaError> {
        let Ok(vals) = outcome else {
            return outcome;
        };
        if co.host_state.is_none() || co.host_stack.is_empty() {
            return Ok(vals);
        }
        // SAFETY: `co` is the coroutine that just returned, held by the
        // caller; its C frames are gone and no reference into it is live
        let mut all = unsafe { co.as_mut() }.host_stack.take().to_vec();
        all.extend(vals);
        Ok(all)
    }
}
