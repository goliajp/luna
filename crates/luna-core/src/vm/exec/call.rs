//! Starting a call: resolving `__call`, pushing a Lua frame or running a
//! native.

use super::*;

mod call_mm;
mod push;

/// `begin_call` of the Lua function `cl`: `tail_ccmt` the count of
/// `__call` metamethods a tail call went through, `chain` the count of
/// those `begin_call` resolved. Returns from the function it is in.
macro_rules! lua_call {
    (
        $vm:ident, $cl:ident, $func_slot:ident, $nargs:ident, $nresults:ident, $from_c:ident,
        $tailcalls:ident, $tail_ccmt:ident, $chain:ident
    ) => {{
        let (cl, func_slot, nargs, nresults, from_c) =
            ($cl, $func_slot, $nargs, $nresults, $from_c);
        let (tailcalls, tail_ccmt, chain) = ($tailcalls, $tail_ccmt, $chain);
        // JIT fast path: if the Proto's body fits
        // the int-arith whitelist, every arg is `Value::Int`,
        // and the cached arity matches, skip frame setup and
        // run the cached native fn in-place.
        let run = $vm.try_jit_call_op(cl, func_slot, nargs, nresults);
        if run != JitRun::NotRun {
            $vm.pending_tailcalls = tailcalls;
            if let Some(e) = $vm.jit.pending_raise.take() {
                $vm.jit.handoff.clear();
                return Err(e);
            }
            if run == JitRun::Done {
                return Ok(false);
            }
            $vm.pending_ccmt = if tailcalls > 0 {
                tail_ccmt
            } else {
                chain as u8
            };
            $vm.jit_resume_handoff(cl, func_slot, nresults, from_c)?;
            return Ok(true);
        }
        $vm.pending_tailcalls = tailcalls;
        $vm.pending_ccmt = if tailcalls > 0 {
            tail_ccmt
        } else {
            chain as u8
        };
        $vm.push_frame(cl, func_slot, nargs, nresults, from_c)?;
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
        if $vm.jit.trace_enabled {
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
            if c >= $vm.jit.call_hot_threshold
                && $vm.jit.active_trace.is_none()
                && !proto.trace_gave_up.get()
                && !proto.trace_call_head_settled.get()
            {
                // The new frame is on top: index in
                // `$vm.frames` is `len() - 1`.
                let frame_idx = $vm.frames.len() - 1;
                // Snapshot R[0..max_stack] at the callee's
                // base. `push_frame` resized `$vm.stack`
                // to `base + max_stack`, so this window is
                // guaranteed in-bounds.
                let f = match &$vm.frames[frame_idx] {
                    CallFrame::Lua(f) => f,
                    _ => unreachable!("push_frame just pushed a Lua frame"),
                };
                let max_stack = cl.proto.max_stack as usize;
                let base_us = f.base as usize;
                let regs = &$vm.stack[base_us..base_us + max_stack];
                if !trace_head_stuck(proto, 0, regs)
                    && !$vm.trace_try_adopt(proto, 0, base_us, None, true)
                {
                    let regs = &$vm.stack[base_us..base_us + max_stack];
                    let entry_tags = regs.iter().map(|v| v.unpack().0).collect();
                    let mut rec =
                        crate::jit::trace::TraceRecord::start(cl.proto, 0, entry_tags, true);
                    rec.settings = $vm.jit.recording_settings();
                    $vm.jit.active_trace = Some(Box::new(rec));
                    $vm.jit.recording_frame_base = frame_idx;
                }
            }
        }
        return Ok(true);
    }};
}

/// `begin_call` of the native `nc`, the counts as in [`lua_call`].
/// Returns from the function it is in.
macro_rules! native_call {
    (
        $vm:ident, $nc:ident, $func_slot:ident, $nargs:ident, $nresults:ident,
        $tail_ccmt:ident, $chain:ident
    ) => {{
        let (nc, func_slot, nargs, nresults) = ($nc, $func_slot, $nargs, $nresults);
        let (tail_ccmt, chain) = ($tail_ccmt, $chain);
        // a C function gets `LUA_MINSTACK` slots (PUC `luaD_precall`)
        if nc.kind == NativeKind::Plain {
            $vm.check_lua_stack(func_slot + 1 + nargs, 20, true)?;
        } else {
            $vm.special_native_room(nc.kind, func_slot + 1 + nargs)?;
        }
        // `pcall`, `xpcall` and an async native are C functions
        // too: 5.2's `luaD_precall` checks before running them
        if matches!(
            nc.kind,
            NativeKind::Pcall | NativeKind::Xpcall | NativeKind::Async
        ) {
            $vm.precall_checkpoint(func_slot + 1 + nargs)?;
        }
        if nc.kind != NativeKind::Plain
            && let Some(r) = $vm.begin_special_native(nc, func_slot, nargs, nresults)
        {
            return r;
        }
        // a native that collects (e.g. `collectgarbage`) roots up to
        // its own arguments — the caller's live registers all sit
        // below `func_slot` and stay rooted.
        $vm.native_nresults = nresults;
        $vm.gc_top = func_slot + nargs + 1;
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
        $vm.push_native(crate::vm::callstack::NativeAct::new(
            nc,
            func_slot,
            nargs,
            $vm.frames.len(),
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
        $vm.gc_top = $vm.gc_top.max(func_slot + 1 + nargs);
        $vm.precall_checkpoint(func_slot + 1 + nargs)?;
        // PUC luaD_precall fires the "call" hook for C functions too.
        // A yield inside the native (coroutine.yield) propagates an
        // Err and the matching "return" hook fires on resume instead.
        if let Err(e) = $vm.hook_call(true, nargs) {
            $vm.running_natives.pop();
            return Err(e);
        }
        let nret = $vm.invoke_native(nc, func_slot, nargs)?;
        $vm.finish_native_call(func_slot, nargs, nret, nresults)?;
        return Ok(false);
    }};
}

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
                // each a function of its own in an unoptimised build, only the
                // one taken then on the stack under the call it makes; the
                // same code in place in an optimised one
                Value::Closure(cl) => {
                    #[cfg(luna_hot)]
                    lua_call!(
                        self, cl, func_slot, nargs, nresults, from_c, tailcalls, tail_ccmt, chain
                    );
                    #[cfg(not(luna_hot))]
                    return self.begin_lua_call(
                        cl, func_slot, nargs, nresults, from_c, tailcalls, tail_ccmt, chain,
                    );
                }
                Value::Native(nc) => {
                    #[cfg(luna_hot)]
                    native_call!(self, nc, func_slot, nargs, nresults, tail_ccmt, chain);
                    #[cfg(not(luna_hot))]
                    return self
                        .begin_native_call(nc, func_slot, nargs, nresults, tail_ccmt, chain);
                }
                v => {
                    chain += 1;
                    self.shift_in_call_mm(v, func_slot, nargs, chain)?;
                    nargs += 1;
                }
            }
        }
    }

    /// [`lua_call`] as a function of its own (an unoptimised build).
    #[cfg(not(luna_hot))]
    #[allow(clippy::too_many_arguments)]
    fn begin_lua_call(
        &mut self,
        cl: Gc<crate::runtime::LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
        tailcalls: u32,
        tail_ccmt: u8,
        chain: u32,
    ) -> Result<bool, LuaError> {
        lua_call!(
            self, cl, func_slot, nargs, nresults, from_c, tailcalls, tail_ccmt, chain
        )
    }

    /// [`native_call`] as a function of its own (an unoptimised build).
    #[cfg(not(luna_hot))]
    fn begin_native_call(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        tail_ccmt: u8,
        chain: u32,
    ) -> Result<bool, LuaError> {
        native_call!(self, nc, func_slot, nargs, nresults, tail_ccmt, chain)
    }
}
