//! To-be-closed variables: registering them, running `__close`
//! handlers yieldably, and the returns that wait on them.

use super::*;

impl Vm {
    /// Register a to-be-closed slot (TBC op / generic-for closing value).
    pub(super) fn register_tbc(&mut self, slot: u32) -> Result<(), LuaError> {
        let v = self.stack[slot as usize];
        if matches!(v, Value::Nil | Value::Bool(false)) {
            return Ok(()); // nil and false are silently ignored
        }
        if self.get_mm(v, Mm::Close).is_nil() {
            // PUC `checkclosemth`: "variable '<name>' got a non-closable
            // value", the name as `luaG_findlocal` gives it — the frame's
            // locvars at this pc, else "(temporary)".
            let f = self.top_frame();
            let reg = slot - f.base;
            let pc = (f.pc as usize).saturating_sub(1);
            let name = crate::vm::objname::getlocalname(&f.closure.proto, reg, pc)
                .unwrap_or("(temporary)");
            return Err(self.rt_err(&format!("variable '{name}' got a non-closable value")));
        }
        // compiled code registers in register order and closes before it
        // registers a slot again; only a crafted chunk (`TBC R0; TBC R0`)
        // breaks that, which PUC's list cannot represent either
        if self.tbc.last().is_some_and(|&s| s >= slot) {
            return Err(self.rt_err("'<close>' state corrupted"));
        }
        self.tbc.push_or_abort(slot);
        Ok(())
    }

    /// Close upvalues and run `__close` handlers for slots ≥ `from`
    /// (handlers in reverse registration order; PUC luaF_close).
    pub(super) fn close_slots(&mut self, from: u32, err: Option<Value>) -> Result<(), LuaError> {
        self.close_from(from);
        // PUC: handlers run in reverse declaration order; an error raised by a
        // handler becomes the error object passed to the remaining ones, and
        // the rest are still closed. The last raised error propagates.
        let mut pending = err;
        let mut result = Ok(());
        let saved_err = self.closing_err;
        // On a normal close the handler runs within the closing function's
        // activation (debug parent = that function); during error unwinding the
        // function's frame is already gone, so the handler sits at the C
        // boundary instead (PUC: luaF_close runs after the ci is restored).
        let error_close = err.is_some();
        while let Some(&s) = self.tbc.last() {
            if s < from {
                break;
            }
            self.tbc.pop();
            let v = self.stack[s as usize];
            if matches!(v, Value::Nil | Value::Bool(false)) {
                continue;
            }
            let mm = self.get_mm(v, Mm::Close);
            if mm.is_nil() {
                // PUC `prepclosingmethod`: the __close metamethod was present
                // at OP_TBC (else we would have errored there) but has since
                // been removed/replaced. Treat as a non-callable target.
                let tn = self.obj_typename(v);
                let e = self.rt_err(&format!(
                    "attempt to call a {tn} value (metamethod 'close')"
                ));
                pending = Some(e.0);
                result = Err(e);
                continue;
            }
            // root the pending error: a handler may trigger a collection
            self.closing_err = pending;
            // PUC `luaF_close` sets `ci->u.l.tm = TM_CLOSE` so traceback /
            // getinfo report the handler as "in metamethod 'close'". Saved/
            // restored around the call to cover the path where `mm` is a
            // native (`push_frame` never consumes it) or it raises before
            // reaching push_frame.
            let saved_tm = self
                .pending_tm
                .replace(crate::runtime::function::FrameTm::Close);
            // PUC 5.4 `prepclosingmethod` always pushed (obj, errobj) — errobj
            // is nil on a normal close (5.4 locals.lua :875's
            // `func2close(coroutine.yield)` wrap pins `(self, nil)` back
            // through the yield). PUC 5.5 dropped the trailing nil: a clean
            // close passes only `obj`, the error case still passes both
            // (5.5 locals.lua :314 `select("#", ...) == n` with n=1 for the
            // normal-close arms, n=2 for the error arm).
            let call = match pending {
                Some(e) => self.call_value_impl(mm, &[v, e], error_close),
                None => {
                    if self.version >= LuaVersion::Lua55 {
                        self.call_value_impl(mm, &[v], error_close)
                    } else {
                        self.call_value_impl(mm, &[v, Value::Nil], error_close)
                    }
                }
            };
            self.pending_tm = saved_tm;
            if let Err(e) = call {
                pending = Some(e.0);
                result = Err(e);
            }
        }
        self.closing_err = saved_err;
        result
    }

    /// Yieldable variant of `close_slots`: drive the chain of `__close`
    /// handlers for slots ≥ `from` through the interpreter loop with a
    /// `Cont::Close` continuation, so a `coroutine.yield()` inside any handler
    /// suspends cleanly (the close iteration's state rides on the thread's
    /// frame/stack like any other suspended call) — PUC's `lua_callk` pattern
    /// applied to `luaF_close`. `after` runs when every slot is closed; if
    /// `after` is `Return` and we've returned past `entry_depth`,
    /// `Ok(Some(vals))` carries the result up to the host caller.
    pub(super) fn begin_close(
        &mut self,
        from: u32,
        err: Option<Value>,
        after: AfterClose,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        self.close_from(from);
        self.drive_close(from, err, after, entry_depth)
    }

    /// Pop tbc slots ≥ `from`, skipping nil/false and synthesising a
    /// non-callable-mm error for an `__close` that was reset to a bad value
    /// between OP_TBC and now (PUC `prepclosingmethod`). The first real
    /// handler pushes a `Cont::Close` + `begin_call` and returns `Ok(None)`;
    /// the interpreter then drives the handler and re-enters this driver via
    /// the `Cont::Close` consumer in `run()`. When the chain is exhausted,
    /// the threaded error (if any) propagates or `after` fires.
    pub(super) fn drive_close(
        &mut self,
        from: u32,
        mut pending: Option<Value>,
        after: AfterClose,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        loop {
            let drained = match self.tbc.last() {
                None => true,
                Some(&s) => s < from,
            };
            if drained {
                return self.finish_close_after(after, pending, entry_depth);
            }
            let s = self.tbc.pop().expect("tbc non-empty");
            let v = self.stack[s as usize];
            if matches!(v, Value::Nil | Value::Bool(false)) {
                continue;
            }
            let mm = self.get_mm(v, Mm::Close);
            if mm.is_nil() {
                let tn = self.obj_typename(v);
                let e = self.rt_err(&format!(
                    "attempt to call a {tn} value (metamethod 'close')"
                ));
                pending = Some(e.0);
                continue;
            }
            // A real handler: stage [mm, v, (err?)] above the current top,
            // record the close iteration state in a Cont::Close, and let the
            // interpreter dispatch the handler. On return the run() head
            // re-enters this driver via the Cont::Close consumer. A threaded
            // error goes in the continuation's own slot first, below the
            // call, where the collector and a suspended thread keep it.
            let func_slot = self.top;
            let error_close = pending.is_some();
            let call_slot = func_slot + error_close as u32;
            let need = (call_slot + 3) as usize;
            if self.stack.len() < need {
                self.stack.resize_or_abort(need, Value::Nil);
            }
            if let Some(e) = pending {
                self.stack[func_slot as usize] = e;
            }
            self.stack[call_slot as usize] = mm;
            self.stack[call_slot as usize + 1] = v;
            // PUC 5.4 always passes (obj, errobj=nil) on a normal close;
            // 5.5 drops the trailing nil. 5.4 locals.lua :875 vs 5.5 :314.
            let nargs = match pending {
                Some(e) => {
                    self.stack[call_slot as usize + 2] = e;
                    2u32
                }
                None => {
                    if self.version >= LuaVersion::Lua55 {
                        1u32
                    } else {
                        self.stack[call_slot as usize + 2] = Value::Nil;
                        2u32
                    }
                }
            };
            self.top = call_slot + 1 + nargs;
            // Root the pending error during the call (a handler may collect).
            let saved_err = self.closing_err;
            self.closing_err = pending;
            // PUC `luaF_close` flags the handler frame as "metamethod 'close'"
            // for traceback / getinfo.
            let saved_tm = self
                .pending_tm
                .replace(crate::runtime::function::FrameTm::Close);
            frames_push_sync(
                &mut self.frames,
                &mut self.frames_top,
                &mut self.trap,
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Close(CloseCont {
                        from,
                        has_pending: error_close,
                        after,
                    }),
                    func_slot,
                    nresults: 0,
                }),
            );
            // PUC luaF_close runs a normal close *within* the closing
            // function's activation (debug parent = that function); during an
            // error unwind the function's frame is already gone and the
            // handler sits at the C boundary instead.
            let r = self.begin_call(call_slot, Some(nargs), 0, error_close);
            self.pending_tm = saved_tm;
            self.closing_err = saved_err;
            r?;
            return Ok(None);
        }
    }

    /// Fire `after` once every `__close` handler has run. `Block` propagates
    /// any remaining error or simply continues; `Return` performs OP_Return's
    /// tail (hook + frame pop + result delivery) and may surface results to
    /// the host when the function whose return triggered the close was the
    /// entry activation, but only on a clean drain — a pending error skips
    /// the return tail and propagates instead. `ResumeUnwind` pops the
    /// deferred Lua frame and re-raises, letting a handler's own error win
    /// over the original propagating one (PUC luaF_close).
    pub(super) fn finish_close_after(
        &mut self,
        after: AfterClose,
        pending: Option<Value>,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        match after {
            AfterClose::Block => match pending {
                Some(e) => Err(LuaError(e)),
                None => Ok(None),
            },
            AfterClose::Return {
                abs_a,
                nret,
                from_native,
            } => match pending {
                Some(e) => Err(LuaError(e)),
                None => self.complete_return(abs_a, nret, from_native, entry_depth),
            },
            AfterClose::ResumeUnwind { func_slot } => {
                // The aborting Lua frame was popped before `begin_close`;
                // restore the catcher's stack window down to `func_slot` and
                // re-raise the threaded error, which started as the original
                // one and is the last a handler raised (PUC luaF_close).
                self.stack.truncate(func_slot as usize);
                self.top = func_slot;
                self.tbc.retain(|&s| s < func_slot);
                let Some(e) = pending else {
                    unreachable!("an unwinding close always threads an error")
                };
                Err(LuaError(e))
            }
        }
    }

    /// OP_Return's post-close tail: fire the "return" hook (frame still
    /// current), pop the Lua frame, slide results into `func_slot`, then
    /// either hand them to the host (`Ok(Some(vals))` when we've returned
    /// past `entry_depth`), leave them contiguous for an exposed
    /// pcall/xpcall continuation, or finish into the caller's expected
    /// result slot. Mirrors the synchronous OP_Return tail so both paths
    /// share semantics — the `from_native` flag selects the right "return"
    /// hook context for `hook_return`.
    pub(super) fn complete_return(
        &mut self,
        abs_a: u32,
        nret: u32,
        from_native: bool,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // ftransfer is the local index (1-based) of the first result, as
        // `getinfo("r").ftransfer + getlocal(level, k)` consumes it. luna
        // exposes locals starting at `frame.base` (= func_slot + 1 +
        // n_varargs for a vararg call), so the conversion is the absolute
        // result slot minus base, plus one to make it 1-based. db.lua 5.4
        // :542 (`foo1(); on=false; eqseq(out, {10, 0})`) pins the vararg
        // shape end-to-end.
        let ftransfer = self
            .frames
            .last()
            .and_then(CallFrame::lua)
            .map(|fr| {
                let raw = abs_a.saturating_sub(fr.base) + 1;
                // 5.5 anonymous-vararg functions get a `(vararg table)` pseudo
                // local injected at index `numparams + 1`, so getlocal
                // numbering shifts results past it (5.5 db.lua :539
                // `eqseq(out, {10, 0})`). 5.4 and earlier have no such pseudo.
                if fr.closure.proto.has_vararg_table_pseudo {
                    raw + 1
                } else {
                    raw
                }
            })
            .unwrap_or(1);
        // PUC 5.1 `luaD_poscall`: fire one extra "tail return" hook event
        // per tail call that collapsed into this activation, *after* its
        // own "return". `tailcalls` tracks that count exactly (PUC
        // `ci->u.l.tailcalls`). 5.2+ retired LUA_HOOKTAILRET, so the
        // "return" hook fires once even when the activation absorbed
        // multiple tail calls — only `istailcall` on getinfo surfaces the
        // collapse. 5.1 db.lua :366 pins the event ordering.
        let tailcalls = if self.version <= LuaVersion::Lua51 {
            self.frames
                .last()
                .and_then(|f| f.lua())
                .map(|f| f.tailcalls)
                .unwrap_or(0)
        } else {
            0
        };
        self.hook_return(from_native, ftransfer, nret)?;
        for _ in 0..tailcalls {
            self.hook_tail_return()?;
        }
        let CallFrame::Lua(fr) =
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap)
                .expect("no frame")
        else {
            unreachable!("returning from a non-Lua frame")
        };
        for i in 0..nret {
            self.stack[(fr.func_slot + i) as usize] = self.stack[(abs_a + i) as usize];
        }
        if self.frames.len() < entry_depth {
            self.top = fr.func_slot + nret;
            return Ok(Some(self.take_results(fr.func_slot)));
        } else if matches!(self.frames.last(), Some(CallFrame::Cont(_))) {
            self.top = fr.func_slot + nret;
        } else {
            self.finish_results(fr.func_slot, nret, fr.nresults);
        }
        Ok(None)
    }
}
