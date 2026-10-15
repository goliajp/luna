//! Collector pacing, the root set and full collections.

use super::*;
mod gc_verify;

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
                self.clear_dead_stack();
                self.full_collect();
            }
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("x"));
        }
        prev
    }

    /// The standalone interpreter switching the collector to generational
    /// mode before it runs anything (5.4+ `lua.c`).
    #[doc(hidden)]
    pub fn gc_enter_generational(&mut self) {
        if self.version >= LuaVersion::Lua54 {
            self.gc_switch_mode("generational");
        }
    }

    /// Whether the current `collectgarbage` mode is "generational" (where a "step"
    /// is a minor collection — a full atomic pass — rather than a paced sweep).
    pub(crate) fn gc_mode_is_generational(&self) -> bool {
        self.gc_mode == "generational"
    }

    /// Current `stepsize` pacing parameter (PUC: 0 means an unbounded step that
    /// completes a whole cycle at once).
    pub(crate) fn gc_stepsize(&self) -> i64 {
        self.gc_stepsize
    }

    /// Set luna's collector knobs: heap growth before a new cycle (%), sweep
    /// work per safe point, and the default step size (0 = a step completes
    /// the cycle).
    pub(crate) fn set_gc_pacing(&mut self, pause: i64, stepmul: i64, stepsize: i64) {
        self.gc_pause = pause;
        self.gc_stepmul = stepmul;
        self.gc_stepsize = stepsize;
    }

    /// A point where PUC checks whether a collection step is due
    /// (`luaC_checkGC`), with PUC's stack top there (luna's numbering):
    /// the slots from `live_top` up are dead to a collection that runs.
    ///
    /// Running Propagate from a checkpoint relies on objects being **born
    /// black during Propagate**: a newly allocated object never becomes
    /// dead-white at the atomic flip.
    ///
    /// Adaptive budget scales with heap size: 100M-object heap (heavy.lua's
    /// `loadrep` stress) gets a 25M-object budget so a cycle completes in
    /// O(SWEEP_DIVISOR) checkpoints regardless of size.
    #[inline(always)]
    pub(crate) fn checkpoint(&mut self, live_top: u32) {
        #[cfg(feature = "gc-trace")]
        self.trace_checkpoint();
        #[cfg(feature = "gc-stress")]
        if !self.heap.gc_is_stopped() {
            self.heap.safe_point();
            self.gc_top = live_top;
            self.collect_garbage();
            return;
        }
        // everything live is rooted here; what is allocated next is young
        self.heap.safe_point();
        if !self.heap.gc_due() || self.gc_finalizing {
            return;
        }
        // Bare `live_top`, no `max(self.top)` widening: every frame-pop site
        // (`finish_results`, the Op::TailCall collapse, pcall unwind) clears
        // the slots it vacates, mirroring PUC's L->top discipline.
        self.gc_top = live_top;
        // PUC stepmul: % of allocation rate. Higher = more GC work per
        // safe-point (lower memory, more CPU). Default 100 = `live / 4` per
        // step (~4 safe-points per cycle). stepmul=200 → `live / 2`, etc.
        const SWEEP_BASE: usize = 400; // 400 / stepmul=100 = divisor 4
        const MIN_BUDGET: usize = 64_000;
        let stepmul = self.gc_stepmul.max(1) as usize;
        let divisor = (SWEEP_BASE / stepmul).max(1);
        let budget = (self.heap.live_objects() / divisor).max(MIN_BUDGET);
        if self.gc_step(budget) {
            self.heap.rearm_gc_pause(self.gc_pause);
        }
    }

    /// The running stack's contract with the collector (PUC
    /// `traversethread`): the slots from `gc_top` up are dead when a cycle's
    /// marking ends, so they are cleared right then, before the sweep frees
    /// anything they point to. The other threads' stacks are marked whole.
    /// So every slot of every stack holds nil or a live value.
    pub(super) fn clear_dead_stack(&mut self) {
        let lo = (self.gc_top as usize).min(self.stack.len());
        self.stack[lo..].fill(Value::Nil);
        self.collector_shrinks_stacks();
    }

    /// Mark the GC roots in `m`: first-class `Value` roots plus bare-object
    /// roots (open upvalues, which are not first-class Values). Shared by the
    /// full collector and the incremental driver so both mark the exact
    /// same live set. Marks in place, without collecting the roots first, so
    /// a collection needs no memory for them.
    pub(crate) fn mark_roots(&self, m: &mut crate::runtime::heap::Marker) {
        m.value(Value::Table(self.globals));
        for mt in self.type_mt.into_iter().flatten() {
            m.value(Value::Table(mt));
        }
        for &n in self.mm_names.iter().chain(&self.heap.puc.pins) {
            m.value(Value::Str(n));
        }
        // Root the running thread's live registers (PUC marks [stack, top)).
        // `gc_top` is the instruction-level cursor of the last GC
        // safe-point: allocation safe-points set it via
        // `checkpoint(live_top)`, and `begin_call` raises it
        // to the callee's argument top when entering a native — PUC's
        // `L->top = func + 1 + nargs` C-call discipline. Without that
        // raise, an explicit `collectgarbage()` collected with a STALE
        // cursor from some earlier (lower) safe-point and freed its own
        // caller's register-held strings
        // (STATUS_ACCESS_VIOLATION on Windows / ASAN heap-use-after-free
        // on Linux). Values stranded above the cursor stay
        // excluded so weak-table entries are not spuriously pinned
        // (gc.lua:544 suspended-coroutine collection).
        let live = (self.gc_top as usize).min(self.stack.len());
        for &v in self.stack[..live].iter().chain(self.jit.ssa_roots.iter()) {
            m.value(v);
        }
        for cf in &self.frames {
            match cf {
                CallFrame::Lua(f) => {
                    m.value(Value::Closure(f.closure));
                }
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Xpcall { handler, .. },
                    ..
                }) => {
                    m.value(*handler);
                }
                // a close chain's threaded error sits on the stack below its
                // handler's call, inside the live window
                CallFrame::Cont(_) => {}
            }
        }
        if let Some(e) = self.closing_err {
            m.value(e);
        }
        for &v in self.host_light.values() {
            m.value(v);
        }
        // Host roots — Lua-facade handles keep their referenced
        // values alive across calls/yields. Trace the whole vector;
        // unused slots (post-`unpin_all`) carry Value::Nil which the
        // GC ignores.
        for slot in &self.host_roots {
            // free-list slots carry Value::Nil (GC no-op)
            m.value(slot.value);
        }
        // `table.sort` and similar builtins stash their working
        // `Vec<Value>` here so a `collectgarbage()` invoked inside the
        // comparator callback doesn't free strings/tables snapshotted
        // off the live table (sort.lua's `load(..)(); collectgarbage()`
        // compare regression).
        for &v in self.sort_scratch.iter().flatten().chain(&self.lib_roots) {
            m.value(v);
        }
        // The running-natives chain holds Gc<NativeClosure>s
        // mid-execution. Without rooting them here, a `collectgarbage()`
        // invoked inside the running native (sort.lua's `load(..)();
        // collectgarbage()` compare callback regression) sweeps the
        // closure that's actively executing, leaving `nc.upvals`
        // dangling and the Rust local `nc` pointing at recycled memory
        // — the SIGSEGV pops on the very next field access or pop.
        for a in &self.running_natives {
            m.value(Value::Native(a.nc));
        }
        // the running thread's debug hook (suspended threads root theirs via
        // Coro::trace / the main_ctx sweep below)
        if let Some(h) = self.hook.func {
            m.value(h);
        }
        // the running coroutine (its saved-context fields live in the VM, but
        // the object itself + its resumer chain must stay reachable)
        if let Some(co) = self.current {
            m.value(Value::Coro(co));
        }
        if let Some(mc) = self.main_coro {
            m.value(Value::Coro(mc));
        }
        // debug.getregistry() and io library state
        if let Some(r) = self.registry {
            m.value(Value::Table(r));
        }
        if let Some(mt) = self.file_mt {
            m.value(Value::Table(mt));
        }
        if let Some(f) = self.io_input {
            m.value(Value::Userdata(f));
        }
        if let Some(f) = self.io_output {
            m.value(Value::Userdata(f));
        }
        if let Some(f) = self.io_stdin {
            m.value(Value::Userdata(f));
        }
        // the main thread's saved context while a coroutine runs
        if let Some(mc) = &self.main_ctx {
            for &v in mc.stack.iter() {
                m.value(v);
            }
            if let Some(h) = mc.hook.func {
                m.value(h);
            }
            for cf in mc.frames.iter() {
                match cf {
                    CallFrame::Lua(f) => {
                        m.value(Value::Closure(f.closure));
                    }
                    CallFrame::Cont(NativeCont {
                        kind: ContKind::Xpcall { handler, .. },
                        ..
                    }) => {
                        m.value(*handler);
                    }
                    CallFrame::Cont(_) => {}
                }
            }
        }
        for &(_, uv) in self.open_upvals.iter() {
            m.mark(uv);
        }
        if let Some(mc) = &self.main_ctx {
            for &(_, uv) in mc.open_upvals.iter() {
                m.mark(uv);
            }
        }
    }

    /// A full stop-the-world collection with the VM's roots (PUC
    /// `luaC_fullgc`), finishing an incremental cycle in progress first.
    /// Returns the number of objects freed.
    pub(crate) fn full_collect(&mut self) -> usize {
        if self.heap.gc_phase_is_propagate() {
            let mut m = self.heap.loan_marker();
            self.mark_roots(&mut m);
            self.heap.stash_marker(m);
            self.watch_errors();
            self.heap.gc_finish_atomic();
            self.collector_shrinks_threads();
            self.forget_dead_errors();
        }
        self.heap.gc_finish_sweep();
        let mut m = self.heap.gc_begin_full();
        self.mark_roots(&mut m);
        self.watch_errors();
        let freed = self.heap.gc_end_full(m);
        self.collector_shrinks_threads();
        self.forget_dead_errors();
        freed
    }

    /// Run a full collection with the VM's roots, then run any `__gc`
    /// finalizers the collection scheduled. A no-op (returns 0) when already
    /// inside a finalizer — the collector is not reentrant (PUC).
    pub fn collect_garbage(&mut self) -> usize {
        if self.gc_finalizing {
            return 0;
        }
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("X"));
        let mut freed = 0;
        if !self.heap.no_gc() {
            self.clear_dead_stack();
            freed = self.full_collect();
            #[cfg(feature = "gc-verify")]
            self.verify_frame_regs_live("collect_garbage");
            self.run_finalizers();
        }
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("x"));
        freed
    }

    /// PUC 5.1 `collectgarbage` re-raised the first error a `__gc` finalizer
    /// threw; gc.lua's "errors during collection" probe relies on it. This
    /// variant runs the same cycle but propagates the captured finalizer
    /// error to the explicit caller.
    pub(crate) fn collect_garbage_propagating(&mut self) -> Result<usize, LuaError> {
        if self.gc_finalizing {
            return Ok(0);
        }
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("X"));
        let mut freed = 0;
        if !self.heap.no_gc() {
            self.clear_dead_stack();
            freed = self.full_collect();
            #[cfg(feature = "gc-verify")]
            self.verify_frame_regs_live("collect_garbage_propagating");
            let r = self.run_finalizers_or_err(true);
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("x"));
            r?;
            return Ok(freed);
        }
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("x"));
        Ok(freed)
    }

    /// What `collectgarbage("count")` and `lua_gc(LUA_GCCOUNT)` report:
    /// PUC's `totalbytes` (see `heap_acct.rs`).
    pub(crate) fn gc_count_bytes(&self) -> usize {
        self.heap.bytes()
    }

    /// Write a mark into the accounting trace (`gc-trace`), where a test
    /// cuts the comparison with PUC's.
    #[cfg(feature = "gc-trace")]
    #[doc(hidden)]
    pub fn trace_mark(&mut self, name: &str) {
        self.heap.trace.line(format_args!("M {name}"));
        // `R38_EXIT_AT=<name>`: stop the program at this mark
        if std::env::var("R38_EXIT_AT").is_ok_and(|at| at == name) {
            self.heap.trace.flush();
            std::process::exit(0);
        }
    }

    /// A checkpoint in the accounting trace: `k` and the running Lua
    /// function's current line, 0 in a native.
    #[cfg(feature = "gc-trace")]
    fn trace_checkpoint(&mut self) {
        let line = match self.frames.last() {
            Some(CallFrame::Lua(f)) if !self.native_on_top() => {
                crate::vm::callstack::frame_line(f).max(0)
            }
            _ => 0,
        };
        self.heap.trace.line(format_args!("k {line}"));
    }

    /// Whether a `__gc` finalizer is currently running (so `collectgarbage`
    /// should report fail rather than collect).
    pub(crate) fn gc_is_finalizing(&self) -> bool {
        self.gc_finalizing
    }
}
