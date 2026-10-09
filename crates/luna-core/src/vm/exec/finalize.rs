//! Queuing and running `__gc` finalizers, and the incremental step.

use super::*;

impl Vm {
    /// Register a table for finalization if its (just-set) metatable carries a
    /// `__gc` metamethod (PUC luaC_checkfinalizer at setmetatable time — adding
    /// `__gc` to the metatable afterwards does not retroactively register).
    pub(crate) fn check_finalizer(&mut self, t: Gc<Table>) {
        // Tables gained finalizers in 5.2; PUC 5.1 runs `__gc` for userdata only.
        if self.version == crate::version::LuaVersion::Lua51 {
            return;
        }
        if !self.get_mm(Value::Table(t), Mm::Gc).is_nil() {
            self.heap.register_finalizable(t);
        }
    }

    /// Same as [`Self::check_finalizer`] for a userdata (the C API's
    /// `lua_setmetatable` from 5.2 on). PUC 5.1 instead marks every
    /// userdata that gets a metatable and looks for `__gc` when it is
    /// collected, as `newproxy` does.
    pub(crate) fn check_finalizer_userdata(&mut self, u: Gc<crate::runtime::Userdata>) {
        if !self.get_mm(Value::Userdata(u), Mm::Gc).is_nil() {
            self.heap.register_finalizable_userdata(u);
        }
    }

    /// Run pending `__gc` finalizers (objects the collector resurrected for
    /// finalization). Finalizer errors are swallowed — PUC turns them into a
    /// warning; they must never propagate to the mutator. Reentrancy-guarded.
    pub(super) fn run_finalizers(&mut self) {
        let _ = self.run_finalizers_or_err();
    }

    pub(super) fn run_finalizers_or_err(&mut self) -> Result<(), LuaError> {
        if self.gc_finalizing {
            return Ok(());
        }
        let pending = self.heap.take_tobefnz();
        if pending.is_empty() {
            return Ok(());
        }
        self.gc_finalizing = true;
        let mut first_err: Option<LuaError> = None;
        for obj in pending {
            let gc = self.get_mm(obj, Mm::Gc);
            // PUC 5.2+ accepts any non-nil `__gc` at setmetatable time to
            // schedule the object for finalization (`__gc = true` is the
            // canonical placeholder); only call it at finalize time when it
            // is actually a function. gc.lua 5.2 :412 wires up exactly this
            // sentinel and then expects no call.
            let callable = matches!(gc, Value::Closure(_) | Value::Native(_));
            if callable {
                // PUC `GCTM` sets `CIST_FIN` on the new ci so
                // `funcnamefromfinalizer` reports `namewhat = "metamethod"`,
                // `name = "__gc"`. luna threads the same outcome through the
                // generic `pending_tm` slot: the Lua frame born from this
                // call consumes it in `push_frame`. Saved/restored around the
                // call in case the handler is a native (which never pops it).
                // Bare event name; `frame_name` / `c_frame_name` add the
                // `"__"` debug prefix for 5.2/5.3, drop it for 5.4+. Matches
                // the convention used by `__close`, `__index`, …
                let saved_tm = self
                    .pending_tm
                    .replace(crate::runtime::function::FrameTm::Gc);
                // PUC `GCTM` runs the finalizer with `luaD_pcall` and no
                // message handler
                if let Err(e) = self.call_protected(gc, &[obj]) {
                    // PUC 5.1 GCTM raised the finalizer's error to the
                    // explicit `collectgarbage()` caller (`gc.lua 5.1 :255`
                    // baselines on `not pcall(collectgarbage)`). 5.2/5.3
                    // wrapped it in `error in __gc metamethod (msg)` first
                    // (`callGCTM` → `luaG_runerror`) but still raised. 5.4
                    // introduced the warning system and switched to "warn
                    // then continue" — never re-raise, just route the
                    // wrapped message through `warn`. gc.lua 5.5 :378 wires
                    // up `_WARN` capture under the `if T then …` block to
                    // baseline on the same wrapped string.
                    if self.version >= LuaVersion::Lua54 {
                        self.warn_error("__gc", e.0);
                    } else if first_err.is_none() {
                        let wrapped = if self.version >= LuaVersion::Lua52 {
                            self.gcmm_raised += 1;
                            let inner = match e.0 {
                                Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                                _ => "no message".to_string(),
                            };
                            let msg = format!("error in __gc metamethod ({inner})");
                            let s = Value::Str(self.heap.intern(msg.as_bytes()));
                            LuaError(s)
                        } else {
                            e
                        };
                        first_err = Some(wrapped);
                    }
                }
                self.pending_tm = saved_tm;
            }
        }
        self.gc_finalizing = false;
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Drive one incremental GC step (PUC `collectgarbage("step", n)`).
    /// Crosses up to three phases per call:
    ///   1. Pause      → seed Propagate (`gc_start_propagate`)
    ///   2. Propagate  → drain gray up to `budget`; on exhaustion run atomic
    ///                   (`gc_finish_atomic` → tobefnz populated; finalizers
    ///                   run via `run_finalizers`) and enter Sweep
    ///   3. Sweep      → `gc_sweep_step` up to (residual) `budget`
    /// Returns true when this call completed the cycle's sweep (back to
    /// Pause). The budget is spent generously across phases — a large `n`
    /// can finish a whole cycle in one call (PUC stop-the-world step).
    pub(crate) fn gc_step(&mut self, budget: usize) -> bool {
        // Re-entry guard: never recurse — `run_finalizers` calls Lua code
        // that may hit a safe point and try to step again. Re-entry was OK
        // under STW (collect_garbage had its own guard) but here the
        // intermediate phase state would corrupt.
        if self.gc_finalizing {
            return false;
        }
        if self.heap.gc_phase_is_pause() {
            let mut m = self.heap.gc_begin_propagate();
            self.mark_roots(&mut m);
            self.heap.stash_marker(m);
        }
        if self.heap.gc_phase_is_propagate() {
            if !self.heap.gc_step_propagate(budget) {
                return false;
            }
            self.clear_dead_stack();
            let mut m = self.heap.loan_marker();
            self.mark_roots(&mut m);
            self.heap.stash_marker(m);
            self.heap.gc_finish_atomic();
            // any __gc scheduled by atomic — run before sweep so a finalizer
            // re-registering `self` re-enters the next cycle, not this sweep
            self.run_finalizers();
        }
        // either we just transitioned, or we entered already in Sweep, or
        // a finalizer started a new cycle (gc_sweep_step is a no-op then)
        self.heap.gc_sweep_step(budget)
    }
}
