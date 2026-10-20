//! A protected call catching an error during the unwind.

use super::*;

impl Vm {
    /// A pcall/xpcall continuation catches `err`: its slot receives
    /// `false, msg` and the caller's register window is restored.
    pub(super) fn unwind_catch(
        &mut self,
        nc: NativeCont,
        err: Value,
        saved_len: usize,
        entry_depth: usize,
    ) -> Unwound {
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        self.cont_popped(false, nc.kind.is_level());
        // the error is caught: 5.1's frame array, grown past its limit for
        // the handler, shrinks back (PUC `restore_stack_limit`)
        self.c_overflow_err = None;
        self.overflow_top = None;
        self.restore_frame_limit();
        let result = match nc.kind {
            ContKind::Pcall { .. } => {
                self.msgh_applied = None;
                err
            }
            // the handler ran where the error was raised (see
            // `raise_to_handler`); one raised past the handler's
            // reach (by the unwind itself) meets it here
            ContKind::Xpcall { handler, .. } => {
                if self.msgh_applied.take().is_some_and(|v| v.raw_eq(err)) {
                    err
                } else {
                    self.call_msgh(handler, err)
                }
            }
            ContKind::Meta(_) | ContKind::Pairs { .. } | ContKind::Close(_) | ContKind::Host(_) => {
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
                match self.alloc_str(b"<no error object>") {
                    Ok(s) => Value::Str(s),
                    Err(e) => e.0,
                }
            } else {
                result
            };
        // the error has been caught (pcall/xpcall): the captured
        // traceback was for that error and is no longer in flight.
        self.error_traceback = None;
        self.dead_snapshot = None;
        let fs = nc.func_slot as usize;
        // the catcher's own window already holds these slots unless the
        // stack shrank under it; with no memory to regrow it the memory
        // error goes on past this catcher
        if let Err(e) = self.grow_stack(fs + 2) {
            return Unwound::Propagated(e);
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
        // every frame below keeps its window, and the results just placed
        // (PUC `luaD_shrinkstack` keeps the stack in use by any frame and
        // up to `L->top`); protected calls nested above a frame's window
        // have their results there
        let restore = self
            .frames
            .iter()
            .filter_map(CallFrame::lua)
            .map(|c| (c.base + c.closure.proto.max_stack as u32) as usize)
            .max()
            .unwrap_or(saved_len)
            .max(self.top as usize);
        if self.stack.len() < restore {
            if let Err(e) = self.grow_stack(restore) {
                return Unwound::Propagated(e);
            }
        } else if self.stack.len() > restore {
            self.stack.truncate(restore);
        }
        // PUC `luaD_pcall` shrinks the stack to what the frames still in
        // use reserve, the protected call's own among them; that closes the
        // room an overflow opened once they fit under the limit again
        let inuse = self.stack_in_use().max(nc.kind.ci_top() + 2);
        self.shrink_running(super::stack_inuse::Shrink::Caught, inuse);
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
