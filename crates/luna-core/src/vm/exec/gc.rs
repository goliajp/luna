//! Collector pacing, the root set and full collections.

use super::*;

impl Vm {
    /// Switch the `collectgarbage` mode, returning the previous mode name.
    pub(crate) fn gc_switch_mode(&mut self, new: &'static str) -> &'static str {
        std::mem::replace(&mut self.gc_mode, new)
    }

    /// Whether the current `collectgarbage` mode is "generational" (where a
    /// "step" is a minor collection — a full atomic pass — rather than a paced
    /// incremental sweep).
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

    /// Interpreter safe-point auto-GC: FULL incremental Propagate + adaptive
    /// paced sweep via `Vm::gc_step`.
    ///
    /// Running Propagate from a safe-point relies on objects being **born
    /// black during Propagate**: a newly allocated object never becomes
    /// dead-white at the atomic flip.
    ///
    /// Adaptive budget scales with heap size: 100M-object heap (heavy.lua's
    /// `loadrep` stress) gets a 25M-object budget so a cycle completes in
    /// O(SWEEP_DIVISOR) safe-points regardless of size.
    #[inline(always)]
    pub(crate) fn maybe_collect_garbage(&mut self, live_top: u32) {
        if !self.heap.gc_due() || self.gc_finalizing {
            return;
        }
        // Bare `live_top`, no `max(self.top)` widening: every frame-pop
        // site (`finish_results`, the Op::TailCall collapse, pcall
        // unwind) clears the slots it vacates, mirroring PUC's L->top
        // discipline.
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
        for &n in &self.mm_names {
            m.value(Value::Str(n));
        }
        // Root the running thread's live registers (PUC marks [stack, top)).
        // `gc_top` is the instruction-level cursor of the last GC
        // safe-point: allocation safe-points set it via
        // `maybe_collect_garbage(live_top)`, and `begin_call` raises it
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
        for &v in self.sort_scratch.iter().flatten() {
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
            self.heap.gc_finish_atomic();
        }
        self.heap.gc_finish_sweep();
        let mut m = self.heap.gc_begin_full();
        self.mark_roots(&mut m);
        self.heap.gc_end_full(m)
    }

    /// Run a full collection with the VM's roots, then run any `__gc`
    /// finalizers the collection scheduled. A no-op (returns 0) when already
    /// inside a finalizer — the collector is not reentrant (PUC).
    pub fn collect_garbage(&mut self) -> usize {
        if self.gc_finalizing {
            return 0;
        }
        self.clear_dead_stack();
        let freed = self.full_collect();
        #[cfg(feature = "gc-verify")]
        self.verify_frame_regs_live("collect_garbage");
        self.run_finalizers();
        freed
    }

    /// `gc-verify`: after a collect, every register slot the
    /// collector just rooted (`[0, max(gc_top, top))` — the same bound
    /// `gc_roots` uses) must hold a live value. A dead value inside the
    /// rooted range means the root snapshot and the sweep disagreed —
    /// a use-after-free waiting to happen. (Slots ABOVE the bound may hold
    /// stale dead values legitimately; the interpreter's contract is
    /// that it writes them before reading.)
    #[cfg(feature = "gc-verify")]
    pub(crate) fn verify_frame_regs_live(&self, ctx: &str) {
        let live = self.heap.debug_live_set();
        let header = |v: Value| -> Option<usize> {
            match v {
                Value::Str(s) => Some(s.as_ptr() as usize),
                Value::Table(t) => Some(t.as_ptr() as usize),
                Value::Closure(c) => Some(c.as_ptr() as usize),
                Value::Native(n) => Some(n.as_ptr() as usize),
                Value::Coro(c) => Some(c.as_ptr() as usize),
                Value::Userdata(u) => Some(u.as_ptr() as usize),
                _ => None,
            }
        };
        let bound = (self.gc_top as usize).min(self.stack.len());
        for i in 0..bound {
            if let Some(h) = header(self.stack[i])
                && !live.contains(&h)
            {
                panic!(
                    "[gc-verify] {ctx}: rooted stack slot {i} (gc_top {}, top {}) \
                         holds a dead value {h:#x} after collect",
                    self.gc_top, self.top,
                );
            }
        }
        // Diagnostic tier: a dead value ABOVE the cursor is only a bug if
        // that register is a named local still in scope (the interpreter
        // WILL read it). Cross-check against the proto's LocVar table.
        for (fi, cf) in self.frames.iter().enumerate() {
            if let CallFrame::Lua(f) = cf {
                let base = f.base as usize;
                let maxs = f.closure.proto.max_stack as usize;
                let hi = (base + maxs).min(self.stack.len());
                let pc = f.pc;
                for i in bound.max(base)..hi {
                    if let Some(h) = header(self.stack[i])
                        && !live.contains(&h)
                    {
                        let reg = (i - base) as u32;
                        if let Some(lv) = f
                            .closure
                            .proto
                            .locvars
                            .iter()
                            .find(|lv| lv.reg == reg && lv.start_pc <= pc && pc < lv.end_pc)
                        {
                            panic!(
                                "[gc-verify] {ctx}: frame {fi} IN-SCOPE LOCAL '{}' \
                                     (reg {reg}, abs {i}, pc {pc}, gc_top {}) holds a \
                                     dead value {h:#x} — live_top cursor excluded a \
                                     live named local",
                                lv.name, self.gc_top,
                            );
                        }
                    }
                }
            }
        }
    }

    /// PUC 5.1 `collectgarbage` re-raised the first error a `__gc` finalizer
    /// threw; gc.lua's "errors during collection" probe relies on it. This
    /// variant runs the same cycle but propagates the captured finalizer
    /// error to the explicit caller.
    pub(crate) fn collect_garbage_propagating(&mut self) -> Result<usize, LuaError> {
        if self.gc_finalizing {
            return Ok(0);
        }
        self.clear_dead_stack();
        let freed = self.full_collect();
        #[cfg(feature = "gc-verify")]
        self.verify_frame_regs_live("collect_garbage_propagating");
        self.run_finalizers_or_err()?;
        Ok(freed)
    }

    /// Whether a `__gc` finalizer is currently running (so `collectgarbage`
    /// should report fail rather than collect).
    pub(crate) fn gc_is_finalizing(&self) -> bool {
        self.gc_finalizing
    }
}
