//! Collector pacing, the root set and full collections.

use super::*;
#[cfg(feature = "gc-verify")]
mod gc_verify;
mod roots;
#[cfg(feature = "gc-trace")]
mod trace_k;

impl Vm {
    /// Switch the `collectgarbage` mode, returning the previous mode name.
    pub(crate) fn gc_switch_mode(&mut self, new: &'static str) -> &'static str {
        let prev = std::mem::replace(&mut self.gc_mode, new);
        self.heap.hook_mode(new == "generational");
        crate::runtime::puc::step::change_mode(self, prev, new);
        prev
    }

    /// The standalone interpreter restarting the collector in generational
    /// mode before it runs anything (5.4+ `lua.c`).
    pub(crate) fn gc_enter_generational(&mut self) {
        if self.heap.lua_c_starts_generational() {
            self.heap.gc_set_stopped(false);
            self.gc_switch_mode("generational");
        }
    }

    /// Whether the `collectgarbage` mode is "generational" (a minor step).
    pub(crate) fn gc_mode_is_generational(&self) -> bool {
        self.gc_mode == "generational"
    }

    /// Set luna's collector knobs: heap growth before a new cycle (%), sweep
    /// work per safe point, and the default step size (0: the whole cycle).
    pub(crate) fn set_gc_pacing(&mut self, pause: i64, stepmul: i64, stepsize: i64) {
        self.gc_pause = pause;
        self.gc_stepmul = stepmul;
        self.gc_stepsize = stepsize;
    }

    /// A point where PUC checks whether a step is due (`luaC_checkGC`), its
    /// stack top there: the slots from `live_top` up are dead to it. An
    /// object made while marking survives if the atomic step reaches it.
    #[inline(always)]
    #[track_caller]
    pub(crate) fn checkpoint(&mut self, live_top: u32) -> Result<(), LuaError> {
        self.refused_growth()?;
        if self.hook_checkpoint(live_top) {
            return Ok(());
        }
        // everything live is rooted here; what is allocated next is young
        self.heap.safe_point();
        if !self.heap.gc_due() {
            return Ok(());
        }
        // Bare `live_top`, no `max(self.top)` widening: every frame-pop site
        // (`finish_results`, the Op::TailCall collapse, pcall unwind) clears
        // the slots it vacates, mirroring PUC's L->top discipline.
        self.gc_top = live_top;
        crate::runtime::puc::step::puc_step(self)
    }

    /// The running stack's contract with the collector (PUC
    /// `traversethread`): the slots from `gc_top` up are dead when a cycle's
    /// marking ends, so they are cleared then, before the sweep frees what
    /// they point to; the main thread's, while a coroutine runs, from the
    /// top it had when the coroutine started; other threads' stacks are
    /// marked whole.
    pub(super) fn clear_dead_stack(&mut self) {
        let lo = (self.gc_top as usize).min(self.stack.len());
        self.stack[lo..].fill(Value::Nil);
        self.clear_saved_main();
    }

    /// Whether this cycle's propagation has shrunk the running thread.
    pub(super) fn running_shrunk(&self) -> bool {
        let co = self.current.or(self.main_coro);
        co.is_some_and(|c| self.heap.shrunk_this_cycle(c.as_ptr() as *mut _))
    }

    /// A full stop-the-world collection with the VM's roots (PUC
    /// `luaC_fullgc`), finishing an incremental cycle in progress first.
    /// Returns the number of objects freed.
    pub(crate) fn full_collect(&mut self) -> usize {
        let freed = crate::runtime::puc::step::full_collect(self, None);
        // there may be objects to be finalized
        self.heap.finalizers_due();
        crate::runtime::puc::step::gc_full_pause(self);
        freed
    }

    /// A full collection luna runs on its own (not the program), with the
    /// finalizers it calls; their errors go nowhere. A no-op (returns 0)
    /// when already inside a finalizer.
    pub fn collect_garbage(&mut self) -> usize {
        let quiet = std::mem::replace(&mut self.fin_quiet, true);
        let r = self.full_gc();
        self.fin_quiet = quiet;
        r.unwrap_or_else(|_| unreachable!("a quiet collection raised"))
    }

    /// PUC `luaC_fullgc` asked for by the program (`lua_gc`): the error of
    /// a 5.1–5.3 finalizer it calls is its own.
    pub(crate) fn collect_garbage_propagating(&mut self) -> Result<usize, LuaError> {
        self.full_gc()
    }

    fn full_gc(&mut self) -> Result<usize, LuaError> {
        if self.gc_finalizing {
            return Ok(0);
        }
        self.heap.trace_line(format_args!("X"));
        let mut freed = 0;
        if !self.heap.no_gc() {
            // 5.2 calls pending finalizers first; its atomic step clears the stack
            if self.heap.full_calls_pending_first() {
                self.call_all_pending(true)?;
            } else {
                self.clear_dead_stack();
            }
            freed = self.full_collect();
            self.hook_frames_live("collect_garbage");
            self.full_gc_finalizers()?;
        }
        crate::runtime::gc_hooks::hook_full_end(self);
        Ok(freed)
    }

    /// What `collectgarbage("count")` and `lua_gc(LUA_GCCOUNT)` report:
    /// PUC's `totalbytes` (see `runtime::puc`).
    pub(crate) fn gc_count_bytes(&self) -> usize {
        self.heap.bytes()
    }

    /// Write a mark into the accounting trace (`gc-trace`), where a test
    /// cuts the comparison with PUC's.
    pub(crate) fn trace_mark(&mut self, name: &str) {
        self.heap.hook_mark(name);
    }

    /// 5.2's `luaD_precall` checks for a collection on every call, the
    /// `CallInfo` made, `L->top` at `top` (a Lua function's whole frame).
    #[inline(always)]
    #[track_caller]
    pub(crate) fn precall_checkpoint(&mut self, top: u32) -> Result<(), LuaError> {
        if self.heap.rules().check_on_call() {
            return self.checkpoint(top);
        }
        Ok(())
    }

    /// A collection check of PUC's C API inside the running C function:
    /// its stack is live up to its top.
    #[inline]
    #[track_caller]
    pub(crate) fn native_checkpoint(&mut self) -> Result<(), LuaError> {
        if self.checkpoint_idle() {
            return Ok(());
        }
        self.native_checkpoint_due()
    }

    #[inline(never)]
    #[track_caller]
    pub(crate) fn native_checkpoint_due(&mut self) -> Result<(), LuaError> {
        let top = self.native_top().unwrap_or_else(|| self.lua_window_end());
        self.checkpoint(top)
    }

    /// A checkpoint at which no collection is due and nothing traces or
    /// checks it: only the safe point is noted.
    #[inline]
    pub(crate) fn checkpoint_idle(&mut self) -> bool {
        let quiet = !crate::runtime::gc_hooks::CHECKPOINTS_WATCHED;
        if quiet && !self.heap.gc_due() {
            self.heap.safe_point();
            return true;
        }
        false
    }

    /// 5.1's `luaD_call` checks for a collection when the call returns,
    /// its results below `top` (calls from C, metamethods, `pcall`'s).
    pub(crate) fn call_return_checkpoint(&mut self, top: u32) -> Result<(), LuaError> {
        if self.heap.rules().check_on_return() {
            return self.checkpoint(top);
        }
        Ok(())
    }

    /// Whether a `__gc` finalizer is currently running (so `collectgarbage`
    /// should report fail rather than collect).
    pub(crate) fn gc_is_finalizing(&self) -> bool {
        self.gc_finalizing
    }
}
