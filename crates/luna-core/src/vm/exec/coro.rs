//! Coroutine bookkeeping: status queries, slots of another thread,
//! closing a coroutine.

use super::*;

impl Vm {
    // ---- coroutines ----

    pub(crate) fn new_coro(&mut self, body: Value) -> Gc<Coro> {
        // The new coroutine inherits the creating thread's current globals
        // (PUC `lua_newthread`: the new state copies `g->mainthread`'s
        // `l_gt`). `Vm.globals` always reflects the live thread, so reading
        // it here picks the creator regardless of which coro is running.
        let co = self.heap.new_coro(body, self.globals);
        // a new thread inherits the creator's C hook (PUC `lua_newthread`); a
        // Lua hook stays with its thread, as PUC's hook table keeps it
        if matches!(self.hook.func, Some(Value::LightUserdata(_))) {
            let mut h = self.hook;
            h.rust_func = None;
            h.count_left = h.count_base;
            // SAFETY: `co` was just allocated and nothing else refers to it
            unsafe { co.as_mut() }.hook = h;
        }
        if let Some(h) = self.host_cont_hooks {
            (h.created)(self, co);
        }
        co
    }

    /// Is `t` the thread whose context is currently live in the VM?
    pub(crate) fn is_current_thread(&self, t: Option<Gc<Coro>>) -> bool {
        match (self.current, t) {
            (None, None) => true,
            (Some(a), Some(b)) => a.ptr_eq(b),
            _ => false,
        }
    }

    /// Read an open-upvalue slot from its owning thread's stack (the live VM
    /// stack if that thread is current, else its saved context).
    #[doc(hidden)]
    pub fn read_slot(&self, slot: u32, thread: Option<Gc<Coro>>) -> Value {
        let s = slot as usize;
        if self.is_current_thread(thread) {
            self.stack[s]
        } else {
            match thread {
                Some(co) => co.stack[s],
                None => self.main_ctx.as_ref().expect("main context").stack[s],
            }
        }
    }

    pub(super) fn write_slot(&mut self, slot: u32, thread: Option<Gc<Coro>>, v: Value) {
        let s = slot as usize;
        if self.is_current_thread(thread) {
            self.stack[s] = v;
        } else {
            match thread {
                Some(co) => {
                    // SAFETY: `co` is a thread the caller holds and not the running one, so the Vm holds no reference into its saved stack; the caller passes a slot inside it, and the borrow covers one store
                    unsafe { co.as_mut() }.stack[s] = v;
                    // co.stack is traced by Coro::trace; demote co back to
                    // gray so propagate re-traces this slot if it was
                    // already black.
                    self.heap.barrier_back(co);
                }
                None => self.main_ctx.as_mut().expect("main context").stack[s] = v,
            }
        }
    }

    /// Whether `co` is the main thread's identity object.
    pub(crate) fn is_main_coro(&self, co: Gc<Coro>) -> bool {
        self.main_coro.is_some_and(|m| m.ptr_eq(co))
    }

    /// The status of `co` from the caller's view. The main thread's identity
    /// object has no stored status — it is "running" when nothing else runs,
    /// else "normal" (it resumed the active coroutine).
    pub(crate) fn effective_coro_status(&self, co: Gc<Coro>) -> CoroStatus {
        // a thread the C API has seen counts as dead at its base level only
        // while nothing is on its C stack to run
        if self.host_restartable(co) {
            return if co.host_stack.is_empty() {
                CoroStatus::Dead
            } else {
                CoroStatus::Suspended
            };
        }
        if self.is_main_coro(co) {
            if self.current.is_none() {
                CoroStatus::Running
            } else {
                CoroStatus::Normal
            }
        } else {
            co.status
        }
    }

    /// `coroutine.close` (PUC `lua_closethread`): run the suspended coroutine's
    /// pending to-be-closed `__close` handlers, then mark it dead and drop its
    /// context. Handlers see the coroutine's death error (if it died by error)
    /// or nil; an error they raise propagates out. `Ok(Some(e))` means it died
    /// with error `e` and no handler overrode it; `Err` means a handler raised.
    pub(crate) fn close_coro(&mut self, co: Gc<Coro>) -> Result<Option<Value>, LuaError> {
        // re-entrant close: a __close handler closed its own coroutine while the
        // outer close is mid-flight (its context is live). Report success and let
        // the outer close finish — re-entering the swap would corrupt the stack.
        if self.current.is_some_and(|c| c.ptr_eq(co)) {
            return Ok(None);
        }
        // A chain of coroutines whose `__close` handlers each close the previous
        // one recurses on the C stack (PUC `luaD_callnoyield` in `lua_closethread`).
        // The calling handler's `call_value` has already pushed `nccalls` to the
        // cap, so here it reads as full first — report PUC's "C stack overflow"
        // before the next handler call would surface the plainer "stack overflow".
        if self.g.nccalls >= MAX_C_DEPTH || native_stack::is_low(native_stack::RESERVE) {
            return Err(self.rt_err("C stack overflow"));
        }
        let nccalls = self.g.nccalls;
        // SAFETY: `co` is the coroutine being closed, held by the caller (a native argument) and not the running thread (checked above); the borrow covers one `take`
        let death_err = unsafe { co.as_mut() }.error_value.take();
        // swap the caller's live context out (into a GC-rooted home) and the
        // coroutine's in, mirroring resume_coro, so the __close handlers run on
        // the coroutine's stack while everything stays rooted.
        let resumer = self.current;
        let rctx = self.take_ctx();
        match resumer {
            Some(r) => {
                // SAFETY: `r` is `self.current`, the running coroutine and so a root; no reference into it is live here, and `m` ends before the barrier call
                let m = unsafe { r.as_mut() };
                m.stack = rctx.stack;
                m.frames = rctx.frames;
                m.open_upvals = rctx.open_upvals;
                m.tbc = rctx.tbc;
                m.top = rctx.top;
                m.meta_conts = rctx.meta_conts;
                m.stale_frames = rctx.stale_frames;
                m.stack_extra = rctx.stack_extra;
                m.frame_size = rctx.frame_size;
            }
            None => self.main_ctx = Some(rctx),
        }
        self.load_coro_ctx(co);
        self.g.stale_frames = self.frames.len() as u32;
        self.current = Some(co);
        // PUC `luaE_resetthread` closes with no message handler, whatever
        // xpcall the coroutine was suspended in
        let natives_base = std::mem::replace(&mut self.natives_base, self.running_natives.len());
        let msgh_floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let result = self.close_slots(0, death_err);
        self.natives_base = natives_base;
        self.msgh_floor = msgh_floor;
        // discard the (now-closed) coroutine context and restore the caller
        let _ = self.take_ctx();
        match resumer {
            Some(r) => {
                self.load_coro_ctx(r);
                self.current = Some(r);
            }
            None => {
                let m = self.main_ctx.take().expect("main context saved");
                self.put_ctx(m);
                self.current = None;
            }
        }
        self.g.nccalls = nccalls;
        {
            // SAFETY: `co` is still held by the caller, and its context was swapped back out above (`take_ctx`), so `m` is the only reference into it while it is cleared
            let m = unsafe { co.as_mut() };
            m.status = CoroStatus::Dead;
            m.stack.take();
            m.frames.take();
            m.open_upvals.take();
            m.tbc.take();
            m.top = 0;
            m.meta_conts = 0;
            m.stale_frames = 0;
            m.stack_extra = false;
            m.frame_size = 0;
            m.resume_at = None;
            m.error_value = None;
            m.error_traceback = None;
            m.error_levels = None;
        }
        self.host_thread_reset(co);
        result.map(|()| death_err)
    }

    /// `coroutine.running`: the running thread plus whether it is the main one.
    pub(crate) fn running_thread(&self) -> (Value, bool) {
        match self.current {
            Some(co) => (Value::Coro(co), false),
            None => (Value::Coro(self.main_coro.expect("main coro")), true),
        }
    }

    /// `coroutine.isyieldable([co])`: whether `co` (default: the running
    /// thread) can yield. The main thread never can; any other coroutine can,
    /// a dead one included, as in PUC.
    pub(crate) fn is_yieldable(&self, co: Option<Gc<Coro>>) -> bool {
        match co {
            // PUC's `yieldable` asks only whether the thread is inside a
            // non-yieldable call, so a dead coroutine counts as yieldable
            Some(c) => !self.main_coro.is_some_and(|m| m.ptr_eq(c)),
            // the running thread can yield only outside any non-yieldable C call
            None => self.current.is_some() && self.nny == 0,
        }
    }

    /// Why `coroutine.yield` may not suspend the running thread right now, as a
    /// PUC error message — `None` if it may. Distinguishes "not in a coroutine"
    /// from "inside an unyieldable C call" (sort/gsub callback).
    pub(crate) fn yield_barrier(&self) -> Option<&'static str> {
        // 5.1's pcall/xpcall are plain C calls (no continuations), so a yield
        // below one crosses the boundary like any other; 5.1 also has a single
        // wording for every case, the main thread included.
        // 5.1 also calls every metamethod and generic-for iterator through
        // `luaD_call`, which counts as a C level, so a yield from inside one
        // is refused as well.
        if self.version <= LuaVersion::Lua51 {
            let inside_call = self.frames.iter().enumerate().any(|(i, f)| match f {
                CallFrame::Cont(nc) => matches!(nc.kind, ContKind::Meta(_)),
                CallFrame::Lua(fr) => {
                    fr.tm.is_some()
                        || (i > 0
                            && self.frames[i - 1].lua().is_some_and(|c| {
                                let pc = (c.pc as usize).wrapping_sub(1);
                                c.closure
                                    .proto
                                    .code
                                    .get(pc)
                                    .is_some_and(|ins| ins.op().is_tfor_call())
                            }))
                }
            });
            // 5.1 cannot yield across a pcall or a metamethod call either
            let in_cont = self.frames.iter().any(|f| matches!(f, CallFrame::Cont(_)));
            if self.current.is_none() || self.nny > 0 || in_cont || inside_call {
                return Some("attempt to yield across metamethod/C-call boundary");
            }
            return None;
        }
        if self.current.is_none() {
            Some("attempt to yield from outside a coroutine")
        } else if self.nny > 0 {
            Some("attempt to yield across a C-call boundary")
        } else {
            None
        }
    }

    /// The coroutine whose context is currently live (`None` on the main thread).
    pub(crate) fn current_coro(&self) -> Option<Gc<Coro>> {
        self.current
    }

    /// `coroutine.close()` on the *running* thread (PUC 5.5 close-self): run all
    /// its pending `__close` handlers, then signal termination. The handlers run
    /// here, in place, with the thread still non-yieldable (a yield in one hits
    /// the C-call boundary). The returned sentinel unwinds the Rust stack the
    /// way a yield does — `exec_with` propagates it past any protecting pcall
    /// rather than letting `unwind` catch it — and `resume_coro` turns it into a
    /// clean death (or, if a handler raised, the coroutine's error).
    pub(crate) fn close_running(&mut self) -> LuaError {
        let death = match self.close_slots(0, None) {
            Ok(()) => None,
            Err(e) => Some(e.0),
        };
        self.terminating = Some(death);
        LuaError(Value::Nil)
    }

    /// `coroutine.status` as seen by the caller.
    pub(crate) fn coro_status_str(&self, co: Gc<Coro>) -> &'static str {
        match self.effective_coro_status(co) {
            CoroStatus::Suspended => "suspended",
            CoroStatus::Running => "running",
            CoroStatus::Normal => "normal",
            CoroStatus::Dead => "dead",
        }
    }
}
