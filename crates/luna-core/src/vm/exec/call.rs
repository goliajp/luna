//! Starting a call: resolving `__call`, pushing a Lua frame or running a
//! native.

use super::*;

mod call_mm;
mod push;

impl Vm {
    // ---- frames & calls ----

    /// Begin calling stack[func_slot] with `nargs` (None: up to self.top).
    /// Returns true if a Lua frame was pushed (the dispatch loop continues
    /// there), false if a native completed inline.
    pub(super) fn begin_call(
        &mut self,
        func_slot: u32,
        nargs: Option<u32>,
        nresults: i32,
        from_c: bool,
    ) -> Result<bool, LuaError> {
        let mut nargs = match nargs {
            Some(n) => n,
            None => self.top - (func_slot + 1),
        };
        // Consume `pending_is_tail` at the boundary: a tail-call op sets it
        // only for the immediately-following Lua activation. Native dispatch
        // (or `__call` resolution) below must not let it leak to the next
        // begin_call's frame; restore it just before push_frame for the Lua
        // arm so its meaning is preserved across __call chaining.
        let tailcalls = std::mem::take(&mut self.pending_tailcalls);
        let tail_ccmt = std::mem::take(&mut self.pending_ccmt);
        // resolve __call handlers iteratively (PUC tryfuncTM loop): each handler
        // is inserted before the value so it becomes the first argument, and a
        // chain of `__call` tables resolves down to a real function.
        let mut chain = 0u32;
        loop {
            match self.stack[func_slot as usize] {
                Value::Closure(cl) => {
                    // JIT fast path: if the Proto's body fits
                    // the int-arith whitelist, every arg is `Value::Int`,
                    // and the cached arity matches, skip frame setup and
                    // run the cached native fn in-place.
                    if self.try_jit_call_op(cl, func_slot, nargs, nresults) {
                        self.pending_tailcalls = tailcalls;
                        if let Some(e) = self.jit.pending_raise.take() {
                            return Err(e);
                        }
                        return Ok(false);
                    }
                    self.pending_tailcalls = tailcalls;
                    self.pending_ccmt = if tailcalls > 0 {
                        tail_ccmt
                    } else {
                        chain as u8
                    };
                    self.push_frame(cl, func_slot, nargs, nresults, from_c)?;
                    // Trace-on-call trigger. The frame
                    // we just pushed is the callee whose body the
                    // recorder will trace. Bump the per-Proto call
                    // counter; once it crosses `CALL_HOT_THRESHOLD`
                    // and no other trace is in flight, snapshot the
                    // callee's register window (R[0..max_stack]) and
                    // begin recording at `pc=0`. This is what unlocks
                    // tracing for functions whose body has no negative
                    // `Op::Jmp` back-edge (`fib`, recursive helpers).
                    //
                    // Gated on `trace_jit_enabled`, so the default
                    // dispatch pays a single not-taken branch.
                    if self.jit.trace_enabled {
                        let proto = cl.proto;
                        let c = proto.call_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.call_hot_count.set(c + 1);
                        }
                        // Relaxed call-trigger:
                        // `c >= THRESHOLD` (not `c == THRESHOLD`) +
                        // `!already_cached` short-circuit. Lets a
                        // discarded short call-trigger close retry
                        // on the next call (fib(10/15/20/25)
                        // pathology — first capture is base-case
                        // [Lt,Jmp,Return1]; coverage-heuristic
                        // discards; next call gets to record at a
                        // potentially deeper recursion point).
                        // Without `already_cached`, the relaxed
                        // condition would re-record over a cached
                        // trace every call.
                        //
                        // Additionally short-circuit on
                        // `proto.trace_gave_up`: the per-Proto discard
                        // cap force-compiles a partial trace and flips
                        // it. `trace_call_head_settled` stands for
                        // "a trace is cached at pc 0 or recording it was
                        // abandoned", so no call scans `traces`.
                        if c >= self.jit.call_hot_threshold
                            && self.jit.active_trace.is_none()
                            && !proto.trace_gave_up.get()
                            && !proto.trace_call_head_settled.get()
                        {
                            // The new frame is on top: index in
                            // `self.frames` is `len() - 1`.
                            let frame_idx = self.frames.len() - 1;
                            // Snapshot R[0..max_stack] at the callee's
                            // base. `push_frame` resized `self.stack`
                            // to `base + max_stack`, so this window is
                            // guaranteed in-bounds.
                            let f = match &self.frames[frame_idx] {
                                CallFrame::Lua(f) => f,
                                _ => unreachable!("push_frame just pushed a Lua frame"),
                            };
                            let max_stack = cl.proto.max_stack as usize;
                            let base_us = f.base as usize;
                            let regs = &self.stack[base_us..base_us + max_stack];
                            if !trace_head_stuck(proto, 0, regs)
                                && !self.trace_try_adopt(proto, 0, base_us, None, true)
                            {
                                let regs = &self.stack[base_us..base_us + max_stack];
                                let entry_tags = regs.iter().map(|v| v.unpack().0).collect();
                                let mut rec = crate::jit::trace::TraceRecord::start(
                                    cl.proto, 0, entry_tags, true,
                                );
                                rec.settings = self.jit.recording_settings();
                                self.jit.active_trace = Some(Box::new(rec));
                                self.jit.recording_frame_base = frame_idx;
                            }
                        }
                    }
                    return Ok(true);
                }
                Value::Native(nc) => {
                    // a C function gets `LUA_MINSTACK` slots (PUC `luaD_precall`)
                    self.check_lua_stack(func_slot + 1 + nargs, 20, true)?;
                    // `pcall`, `xpcall` and an async native are C functions
                    // too: 5.2's `luaD_precall` checks before running them
                    if matches!(
                        nc.kind,
                        NativeKind::Pcall | NativeKind::Xpcall | NativeKind::Async
                    ) {
                        self.precall_checkpoint(func_slot + 1 + nargs);
                    }
                    if nc.kind != NativeKind::Plain
                        && let Some(r) = self.begin_special_native(nc, func_slot, nargs, nresults)
                    {
                        return r;
                    }
                    // a native that collects (e.g. `collectgarbage`) roots up to
                    // its own arguments — the caller's live registers all sit
                    // below `func_slot` and stay rooted.
                    self.native_nresults = nresults;
                    self.gc_top = func_slot + nargs + 1;
                    // Push the native onto the running-natives chain BEFORE
                    // firing the call hook so that `debug.getinfo(level)` and
                    // `arg_error` from inside the hook see this native as the
                    // currently-running C function (db.lua :344 reads
                    // `getinfo(2, "f").func` for the just-entered callee).
                    // Popped after the matching return hook fires — even on
                    // error, the pop must happen, so the body is bracketed
                    // through a scope guard.
                    // a tail call resolved its `__call` chain before
                    // calling here and passed the count in tail_ccmt
                    self.push_native(crate::vm::callstack::NativeAct::new(
                        nc,
                        func_slot,
                        nargs,
                        self.frames.len(),
                        tail_ccmt + chain as u8,
                    ))?;
                    // PUC C-call discipline: entering a C function sets
                    // L->top to func + 1 + nargs, so a collect triggered
                    // INSIDE the native (explicit `collectgarbage()`, or
                    // an allocation crossing the GC threshold) roots the
                    // whole caller window up to and including the
                    // arguments. Without this raise the cursor is stale —
                    // parked at some earlier, possibly much lower
                    // safe-point — and the collect frees register-held
                    // values of the native's own caller (use-after-free).
                    // Never lower it: a re-entrant chain
                    // (native → Lua → native) must keep the outermost
                    // window rooted.
                    self.gc_top = self.gc_top.max(func_slot + 1 + nargs);
                    self.precall_checkpoint(func_slot + 1 + nargs);
                    // PUC luaD_precall fires the "call" hook for C functions too.
                    // A yield inside the native (coroutine.yield) propagates an
                    // Err and the matching "return" hook fires on resume instead.
                    if let Err(e) = self.hook_call(true, nargs) {
                        self.running_natives.pop();
                        return Err(e);
                    }
                    let nret = self.invoke_native(nc, func_slot, nargs)?;
                    self.finish_native_call(func_slot, nargs, nret, nresults)?;
                    return Ok(false);
                }
                v => {
                    chain += 1;
                    self.shift_in_call_mm(v, func_slot, nargs, chain)?;
                    nargs += 1;
                }
            }
        }
    }
}
