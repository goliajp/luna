//! Collector pacing, the root set and full collections.

use super::*;
mod gc_verify;
mod roots;
mod trace_k;

impl Vm {
    /// Switch the `collectgarbage` mode, returning the previous mode name.
    /// 5.4 and 5.5 enter generational mode with a full marking and sweep
    /// (`entergen`), which runs no finalizer.
    pub(crate) fn gc_switch_mode(&mut self, new: &'static str) -> &'static str {
        let prev = std::mem::replace(&mut self.gc_mode, new);
        if new == "generational" && prev != new && self.version >= LuaVersion::Lua54 {
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("X"));
            if !self.gc_finalizing && !self.heap.no_gc() {
                self.clear_dead_stack(false);
                self.full_collect();
                crate::runtime::puc::step::gc_minor_debt(self);
            }
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("x"));
        }
        if prev != new && self.version == LuaVersion::Lua52 {
            match new {
                "generational" => crate::runtime::puc::step::enter_generational_52(self),
                _ => crate::runtime::puc::step::leave_generational_52(self),
            }
        }
        if new == "incremental" && prev == "generational" {
            crate::runtime::puc::step::gc_leave_generational(self);
        }
        prev
    }

    /// The standalone interpreter restarting the collector in generational
    /// mode before it runs anything (5.4+ `lua.c`).
    #[doc(hidden)]
    pub fn gc_enter_generational(&mut self) {
        if self.version >= LuaVersion::Lua54 {
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
    #[cfg_attr(feature = "gc-trace", track_caller)]
    pub(crate) fn checkpoint(&mut self, live_top: u32) -> Result<(), LuaError> {
        self.refused_growth()?;
        #[cfg(feature = "gc-trace")]
        self.trace_checkpoint();
        // gc-verify: the live top never cuts into a Lua frame that is running
        #[cfg(feature = "gc-verify")]
        if let Some(CallFrame::Lua(f)) = self
            .frames
            .iter()
            .rev()
            .find(|f| matches!(f, CallFrame::Lua(_)))
            && live_top < f.base
        {
            panic!(
                "[gc-verify] checkpoint with live top {live_top} below the running Lua frame's base {}",
                f.base
            );
        }
        #[cfg(feature = "gc-stress")]
        if !self.heap.gc_is_stopped() {
            self.stress_checkpoint(live_top);
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
    /// marked whole. `shrink`: the stacks shrink as an atomic step's
    /// traversal finds them (a full collection shrinks them in its own).
    pub(super) fn clear_dead_stack(&mut self, shrink: bool) {
        let lo = (self.gc_top as usize).min(self.stack.len());
        self.stack[lo..].fill(Value::Nil);
        self.clear_saved_main();
        // 5.3 shrinks a thread as its propagation reaches it, 5.2 the main
        // one where its sweep ends
        if shrink && !matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            self.collector_shrinks_stacks();
        }
    }

    /// Whether this cycle's propagation has shrunk the running thread.
    fn running_shrunk(&self) -> bool {
        let co = self.current.or(self.main_coro);
        co.is_some_and(|c| self.heap.shrunk_this_cycle(c.as_ptr() as *mut _))
    }

    /// A full stop-the-world collection with the VM's roots (PUC
    /// `luaC_fullgc`), finishing an incremental cycle in progress first.
    /// Returns the number of objects freed.
    pub(crate) fn full_collect(&mut self) -> usize {
        // a sweep (the one in progress, or 5.1's from a pause, which it
        // treats as marking) ends before the full cycle, with `checkSizes`
        let swept = !self.heap.gc_phase_is_pause() || self.version == LuaVersion::Lua51;
        crate::runtime::puc::step::gc_hold_threads(self);
        self.jit_forget_ghosts();
        self.name_cache = Default::default();
        if self.heap.gc_phase_is_propagate() {
            // 5.3 shrinks the running thread as it traverses it, if this
            // cycle has not yet
            if self.version == LuaVersion::Lua53 && !self.running_shrunk() {
                self.collector_shrinks_stacks();
            }
            let mut m = self.heap.loan_marker();
            self.mark_roots(&mut m, true);
            self.heap.stash_marker(m);
            self.watch_errors();
            self.lend_open_lists();
            self.heap.gc_finish_atomic();
            self.collector_shrinks_threads();
            self.forget_dead_errors();
        }
        self.heap.gc_finish_sweep();
        if swept {
            self.heap.acct_check_sizes();
        }
        let freed = if self.version == LuaVersion::Lua52 {
            crate::runtime::puc::step::full_cycle_52(self, swept)
        } else {
            crate::runtime::puc::step::gc_count_marked(self);
            let mut m = self.heap.gc_begin_full();
            self.mark_roots(&mut m, false);
            self.watch_errors();
            self.lend_open_lists();
            self.heap.mark_from(&mut m);
            // the running threads shrink as the cycle traverses them, after
            // the sweep it finished (`checkSizes`), the rest after its own
            // sweep; 5.2's single steps did both
            self.collector_shrinks_stacks();
            let freed = self.heap.gc_sweep_full();
            self.collector_shrinks_threads();
            self.heap.acct_check_sizes();
            freed
        };
        self.forget_dead_errors();
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
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("X"));
        let mut freed = 0;
        if !self.heap.no_gc() {
            // 5.2 calls pending finalizers first; its atomic step clears the stack
            if self.version == LuaVersion::Lua52 {
                self.call_all_pending(true)?;
            } else {
                self.clear_dead_stack(false);
            }
            freed = self.full_collect();
            #[cfg(feature = "gc-verify")]
            self.verify_frame_regs_live("collect_garbage");
            self.full_gc_finalizers()?;
        }
        #[cfg(feature = "gc-trace")]
        crate::runtime::puc::step::trace_full_end(self);
        Ok(freed)
    }

    /// What `collectgarbage("count")` and `lua_gc(LUA_GCCOUNT)` report:
    /// PUC's `totalbytes` (see `runtime::puc`).
    pub(crate) fn gc_count_bytes(&self) -> usize {
        self.heap.bytes()
    }

    /// Write a mark into the accounting trace (`gc-trace`), where a test
    /// cuts the comparison with PUC's.
    #[doc(hidden)]
    pub fn trace_mark(&mut self, name: &str) {
        #[cfg(feature = "gc-trace")]
        {
            self.heap.trace.line(format_args!("M {name}"));
            // `R38_EXIT_AT=<name>`: stop the program at this mark
            if std::env::var("R38_EXIT_AT").is_ok_and(|at| at == name) {
                self.heap.trace.flush();
                std::process::exit(0);
            }
        }
        let _ = name;
    }

    /// 5.2's `luaD_precall` checks for a collection on every call, the
    /// `CallInfo` made, `L->top` at `top` (a Lua function's whole frame).
    #[inline(always)]
    #[cfg_attr(feature = "gc-trace", track_caller)]
    pub(crate) fn precall_checkpoint(&mut self, top: u32) -> Result<(), LuaError> {
        if self.version == crate::version::LuaVersion::Lua52 {
            return self.checkpoint(top);
        }
        Ok(())
    }

    /// A collection check of PUC's C API inside the running C function:
    /// its stack is live up to its top.
    #[inline]
    #[cfg_attr(feature = "gc-trace", track_caller)]
    pub(crate) fn native_checkpoint(&mut self) -> Result<(), LuaError> {
        if self.checkpoint_idle() {
            return Ok(());
        }
        self.native_checkpoint_due()
    }

    #[inline(never)]
    #[cfg_attr(feature = "gc-trace", track_caller)]
    pub(crate) fn native_checkpoint_due(&mut self) -> Result<(), LuaError> {
        let top = self.native_top().unwrap_or_else(|| self.lua_window_end());
        self.checkpoint(top)
    }

    /// A checkpoint at which no collection is due and nothing traces or
    /// checks it: only the safe point is noted.
    #[inline]
    pub(crate) fn checkpoint_idle(&mut self) -> bool {
        let quiet = cfg!(not(any(
            feature = "gc-trace",
            feature = "gc-stress",
            feature = "gc-verify"
        )));
        if quiet && !self.heap.gc_due() {
            self.heap.safe_point();
            return true;
        }
        false
    }

    /// 5.1's `luaD_call` checks for a collection when the call returns,
    /// its results below `top` (calls from C, metamethods, `pcall`'s).
    pub(crate) fn call_return_checkpoint(&mut self, top: u32) -> Result<(), LuaError> {
        if self.version == crate::version::LuaVersion::Lua51 {
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
