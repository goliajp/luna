//! Running the loop to completion and unwinding the stack on an error
//! to the nearest protecting call.

use super::*;

mod catch;
mod exec_with;

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
                        kind: ContKind::Pcall { .. } | ContKind::Xpcall { .. } | ContKind::Close(_),
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
                    self.cont_popped(true, true);
                    self.stack.truncate(mc.saved_len as usize);
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
                    kind: ContKind::Pairs { .. },
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.cont_popped(false, true);
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
                    self.cont_popped(true, true);
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
}
