//! Running the loop to completion and unwinding the stack on an error
//! to the nearest protecting call.

use super::*;

/// Outcome of unwinding the call stack on an error (see `Vm::unwind`).
pub(super) enum Unwound {
    /// caught by a pcall/xpcall continuation; resume running its caller
    Caught,
    /// caught by a continuation that was the entry-level activation; these are
    /// the call's (wrapped) results
    CaughtReturn(Vec<Value>),
    /// no protecting continuation up to `entry_depth`; propagate the error
    Propagated(LuaError),
}

impl Vm {
    // ---- the interpreter ----

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

    pub(super) fn exec_with(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
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

    /// Unwind the call stack from the error point toward `entry_depth`, running
    /// `__close` handlers on each Lua frame. Stops at the first pcall/xpcall
    /// continuation frame at/above `entry_depth` (the error is *caught*: its
    /// slot receives `false, msg`); if none is reached, the error propagates.
    pub(super) fn unwind(&mut self, mut err: Value, entry_depth: usize) -> Unwound {
        // The protected call runs in-place among the caller frames' registers,
        // so truncating the failed frames here cuts into caller windows below
        // the catcher. Snapshot the live length: at the error point the stack
        // already spans every surviving frame's window, so restoring it after a
        // catch reinstates them all (the reclaimed slots above are dead temps).
        // PUC handles overflow recovery via a separate EXTRA_STACK reserve;
        // we instead clamp the restore to the catcher's caller window when the
        // error point was at the stack limit (cause: the next `call_value_impl`
        // picks `func_slot = stack.len()` which would otherwise re-overflow).
        let saved_len = self.stack.len();
        // An error leaving a recording ends that path: what was recorded
        // is not a loop. Closing it later, when the head is reached again
        // through a fresh call, would make a trace that skips the rest of
        // the body and returns to its head for ever. Counted as a failure
        // of that head, so the same doomed recording is not started again
        // and again.
        if let Some(rec) = self.jit.active_trace.take() {
            self.jit.counters.aborted += 1;
            self.jit.counters.bump_close_cause("error-unwind");
            note_trace_compile_failure(rec.head_proto, rec.head_pc);
        }
        err = self.raise_to_handler(err);
        // An error that no protected call inside the running coroutine will
        // catch kills it without unwinding: PUC's `lua_resume` leaves the
        // dead thread's stack as it was, so its pending to-be-closed
        // variables run only when it is closed (`coroutine.close`, or
        // `coroutine.wrap` closing it before re-raising). Scoped to the
        // coroutine's own run (`entry_depth == 1`); a run nested under a
        // native unwinds as before.
        if entry_depth == 1
            && self.version >= LuaVersion::Lua54
            && self
                .current
                .is_some_and(|c| c.status == crate::runtime::CoroStatus::Running)
            && !self.frames.iter().any(|f| {
                matches!(
                    f,
                    CallFrame::Cont(NativeCont {
                        kind: ContKind::Pcall | ContKind::Xpcall { .. } | ContKind::Close(_),
                        ..
                    })
                )
            })
        {
            while self.frames.len() >= entry_depth {
                if let Some(&CallFrame::Cont(NativeCont {
                    kind: ContKind::Host(hc),
                    ..
                })) = self.frames.last()
                {
                    self.discard_host_cont(hc);
                }
                self.pop_frame();
            }
            return Unwound::Propagated(LuaError(err));
        }
        while self.frames.len() >= entry_depth {
            match *self.frames.last().expect("frame") {
                // a yieldable-metamethod continuation does not catch: discard the
                // abandoned instruction and keep unwinding (PUC drops the partial
                // op on error).
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Meta(mc),
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.pcall_depth -= 1;
                    self.stack.truncate(func_slot as usize);
                    self.top = mc.saved_top.min(func_slot);
                    self.tbc.retain(|&s| s < func_slot);
                }
                // a C function's continuation does not catch: the error leaves
                // the C function, whose C API frame goes with it
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Host(hc),
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.stack.truncate(func_slot as usize);
                    self.top = func_slot;
                    self.tbc.retain(|&s| s < func_slot);
                    self.discard_host_cont(hc);
                }
                // a __pairs continuation does not catch either: an error inside
                // the metamethod propagates past `pairs`.
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Pairs,
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.pcall_depth -= 1;
                    self.stack.truncate(func_slot as usize);
                    self.top = func_slot;
                    self.tbc.retain(|&s| s < func_slot);
                }
                // a __close continuation does not catch: drop the half-run
                // handler's window, then continue the close yieldably with
                // the new error threaded as `pending`. Preserve `cc.after`
                // verbatim — `Return`/`Block` originating from an aborting
                // OP_Return/OP_Close will be short-circuited by
                // `finish_close_after` (pending propagates as Err); a
                // `ResumeUnwind` originated by our own Lua-frame handler
                // must keep its deferred frame-pop semantics so that frame
                // is not orphaned. If a fresh handler yields, `drive_close`
                // pushes another `Cont::Close` and we return `Caught` so
                // `exec_with` re-enters the run loop.
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Close(cc),
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.pcall_depth -= 1;
                    self.stack.truncate(func_slot as usize);
                    self.top = func_slot;
                    self.tbc.retain(|&s| s < func_slot);
                    match self.drive_close(cc.from, Some(err), cc.after, entry_depth) {
                        Ok(Some(_)) => {
                            unreachable!(
                                "Block / Return / ResumeUnwind never return host values mid-unwind"
                            )
                        }
                        Ok(None) => return Unwound::Caught,
                        Err(e) => {
                            // the drained close re-raises `err`; only an
                            // error a handler raised is new
                            if !e.0.raw_eq(err) {
                                err = self.raise_to_handler(e.0);
                            }
                            continue;
                        }
                    }
                }
                CallFrame::Cont(nc) => return self.unwind_catch(nc, err, saved_len, entry_depth),
                CallFrame::Lua(f) => {
                    // Yieldable error-unwind close, PUC luaG_errormsg shape:
                    // (1) pop the Lua frame immediately so each `__close`
                    // handler runs at the C boundary above — `debug.getinfo`
                    // sees the next outer Lua frame's call site (typically
                    // `pcall`), not this aborting function (locals.lua:480).
                    // (2) drive the close yieldably with
                    // `AfterClose::ResumeUnwind { func_slot, err }`; on drain
                    // it truncates to `func_slot` and re-raises (letting a
                    // handler-raised error win over `err`). If a handler
                    // yields, `drive_close` pushes `Cont::Close` and we
                    // return `Caught` so `exec_with` re-enters the run loop;
                    // a synchronous drain returns Err exactly as the old
                    // path did.
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    let after = AfterClose::ResumeUnwind {
                        func_slot: f.func_slot,
                    };
                    match self.begin_close(f.base, Some(err), after, entry_depth) {
                        Ok(Some(_)) => {
                            unreachable!("ResumeUnwind never returns host values")
                        }
                        Ok(None) => return Unwound::Caught,
                        Err(e) => {
                            // the drained close re-raises `err`; only an
                            // error a handler raised is new
                            if !e.0.raw_eq(err) {
                                err = self.raise_to_handler(e.0);
                            }
                            continue;
                        }
                    }
                }
            }
        }
        Unwound::Propagated(LuaError(err))
    }

    /// A pcall/xpcall continuation catches `err`: its slot receives
    /// `false, msg` and the caller's register window is restored.
    fn unwind_catch(
        &mut self,
        nc: NativeCont,
        err: Value,
        saved_len: usize,
        entry_depth: usize,
    ) -> Unwound {
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        self.pcall_depth -= 1;
        let result = match nc.kind {
            ContKind::Pcall => {
                self.msgh_applied = None;
                err
            }
            // the handler ran where the error was raised (see
            // `raise_to_handler`); one raised past the handler's
            // reach (by the unwind itself) meets it here
            ContKind::Xpcall { handler } => {
                if self.msgh_applied.take().is_some_and(|v| v.raw_eq(err)) {
                    err
                } else {
                    self.call_msgh(handler, err)
                }
            }
            ContKind::Meta(_) | ContKind::Pairs | ContKind::Close(_) | ContKind::Host(_) => {
                unreachable!("Meta/Pairs/Close/Host cont handled above")
            }
        };
        // PUC 5.5 `luaG_errormsg` substitutes "<no error object>"
        // for nil AFTER the message handler ran (ldebug.c:849) —
        // so it applies to the pcall-caught object and to an
        // xpcall HANDLER'S return value, while the handler itself
        // (and a top-level propagation into the host, whose
        // `error_display` plays msghandler) still sees the raw
        // nil. 5.4- keep nil everywhere (errors.lua :49 asserts
        // `doit("error()") == nil`).
        let result =
            if matches!(result, Value::Nil) && self.version >= crate::version::LuaVersion::Lua55 {
                Value::Str(self.heap.intern(b"<no error object>"))
            } else {
                result
            };
        // the error has been caught (pcall/xpcall): the captured
        // traceback was for that error and is no longer in flight.
        self.error_traceback = None;
        let fs = nc.func_slot as usize;
        if self.stack.len() < fs + 2 {
            self.grow_stack_or_abort(fs + 2);
        }
        self.stack[fs] = Value::Bool(false);
        self.stack[fs + 1] = result;
        self.top = nc.func_slot + 2;
        self.tbc.retain(|&s| s < nc.func_slot);
        if self.frames.len() < entry_depth {
            return Unwound::CaughtReturn(self.take_results(nc.func_slot));
        }
        self.finish_results(nc.func_slot, 2, nc.nresults);
        // reinstate the caller windows the unwind truncated into,
        // clamped to the catcher's caller window + a `MIN_STACK`
        // reserve. The clamp is a no-op for normal pcall catches
        // (saved_len lies within the caller's max_stack window),
        // and prevents the stack from staying near `MAX_LUA_STACK`
        // after an overflow-recovery catch — which would make the
        // next `call_value_impl` (e.g. a `__close` in the catcher's
        // errorh, locals.lua:659) pick `func_slot = stack.len()`
        // above the limit and re-overflow.
        // Restore the caller's full register window: opcodes
        // index it directly. The cap covers caller's base +
        // `max_stack` + a small reserve. We always resize to
        // exactly this window — previously this clamped
        // `saved_len` from above to prevent staying near
        // `MAX_LUA_STACK` after an overflow-recovery catch, and
        // a yieldable-unwind re-entry adds the dual case where
        // `saved_len` is *below* the window (a prior
        // `ResumeUnwind` truncated). Using the window directly
        // covers both.
        let restore = self
            .frames
            .iter()
            .rev()
            .find_map(CallFrame::lua)
            .map(|c| (c.base + c.closure.proto.max_stack as u32) as usize + 256)
            .unwrap_or(saved_len);
        if self.stack.len() < restore {
            self.grow_stack_or_abort(restore);
        } else if self.stack.len() > restore {
            self.stack.truncate(restore);
        }
        // Clear slots vacated by the popped
        // frames the unwind walked over. finish_results
        // above clears `[nc.func_slot + nresults ..
        // nc.func_slot + 2)`, which only covers the
        // pcall's own result region — the unwind-popped
        // frames' locals in `[nc.func_slot + 2 .. restore)`
        // are still in place with whatever Gc-bearing
        // Values they last held. Without this clear, a
        // later GC marks the stale pointers (same hazard as
        // the Op::Return finish_results path). PUC's `luaD_pcall` similarly truncates
        // L->top to the catcher's level — luna's
        // truncate above resizes the Vec but doesn't
        // touch slots [func_slot+2..restore) that were
        // already present.
        let clear_lo = (nc.func_slot as usize + 2).min(self.stack.len());
        let clear_hi = restore.min(self.stack.len());
        if clear_lo < clear_hi {
            for slot in &mut self.stack[clear_lo..clear_hi] {
                *slot = Value::Nil;
            }
        }
        Unwound::Caught
    }
}
