//! Starting a call: resolving `__call`, pushing a Lua frame or running a
//! native.

use super::*;

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
                            let mut entry_tags = Vec::with_capacity(max_stack);
                            for i in 0..max_stack {
                                let (tag, _) = self.stack[base_us + i].unpack();
                                entry_tags.push(tag);
                            }
                            self.jit.active_trace =
                                Some(Box::new(crate::jit::trace::TraceRecord::start(
                                    cl.proto, 0, entry_tags, true,
                                )));
                            self.jit.recording_frame_base = frame_idx;
                        }
                    }
                    return Ok(true);
                }
                Value::Native(nc) => {
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
                    self.running_natives.push(crate::vm::callstack::NativeAct {
                        nc,
                        func_slot,
                        nargs,
                        depth: self.frames.len() as u32,
                        // a tail call resolved its `__call` chain before
                        // calling here and passed the count in tail_ccmt
                        ccmt: tail_ccmt + chain as u8,
                    });
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
                    let mm = self.get_mm(v, Mm::Call);
                    if mm.is_nil() || self.call_mm_unusable(mm) {
                        return Err(self.call_err(v));
                    }
                    chain += 1;
                    // PUC 5.5 dropped the chain cap from `MAXTAGRECUR = 200`
                    // (the value 5.4's `lvm.c` uses) down to `MAXCCMT = 16`,
                    // and the 5.5 test exercises the new tight bound directly
                    // (calls.lua :225 builds a 16-deep chain and expects the
                    // 16th to error). 5.4 calls.lua :194 instead builds a 20-
                    // deep chain and expects it to succeed.
                    let cap = if self.version >= crate::version::LuaVersion::Lua55 {
                        15
                    } else {
                        MAX_CCMT
                    };
                    if chain > cap {
                        return Err(self.rt_err("'__call' chain too long"));
                    }
                    // the callee and its arguments (and anything up to top)
                    // shift up by one (PUC tryfuncTM); slots above them are
                    // dead temps, and inserting into the whole stack would
                    // move all of them and grow it on every hop
                    let from = func_slot as usize;
                    let end = (func_slot + 1 + nargs).max(self.top) as usize;
                    if self.stack.len() <= end {
                        self.stack.resize(end + 1, Value::Nil);
                    }
                    self.stack.copy_within(from..end, from + 1);
                    self.stack[from] = mm;
                    if self.top > func_slot {
                        self.top += 1;
                    }
                    nargs += 1;
                }
            }
        }
    }

    /// Up to 5.3 `tryfuncTM` takes one `__call` hop and needs a function
    /// there; anything else is a call error on the original object. 5.4
    /// retries the call with whatever `__call` holds, so chains resolve.
    pub(super) fn call_mm_unusable(&self, mm: Value) -> bool {
        self.version <= LuaVersion::Lua53 && !matches!(mm, Value::Closure(_) | Value::Native(_))
    }

    pub(super) fn push_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        if func_slot + 256 > MAX_LUA_STACK {
            // PUC `luaD_growstack`: the overflow raises "stack overflow" and
            // leaves ERRORSTACKSIZE's extra slots for the xpcall handler that
            // runs on it; overflowing those is LUA_ERRERR, "error in error
            // handling" (errors.lua :606, cstack.lua :29).
            if self.msgh_depth == 0 {
                return Err(self.rt_err("stack overflow"));
            }
            if func_slot + 256 > MAX_LUA_STACK + ERROR_STACK_EXTRA {
                return Err(self.plain_err("error in error handling"));
            }
        }
        let proto = cl.proto;
        let nparams = proto.num_params as u32;
        // 5.5 vararg layout (PUC luaT_adjustvarargs): the extra args stay on the
        // stack just below the new `base`, so a named vararg can be indexed
        // virtually without allocating a table. Rotate `[p1..pn][e1..em]` to
        // `[e1..em][p1..pn]` so the fixed params land at the new base.
        let n_varargs = nargs.saturating_sub(nparams) * u32::from(proto.is_vararg);
        if n_varargs > 0 {
            let s = (func_slot + 1) as usize;
            self.stack[s..s + nargs as usize].rotate_left(nparams as usize);
        }
        let base = func_slot + 1 + n_varargs;
        let max = proto.max_stack as u32;
        let need = (base + max) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        // Only missing parameters become nil (PUC `luaD_precall`): the code
        // writes the rest before reading it, the collector keeps it valid
        // (`clear_dead_stack`) and a trace checks only what it reads first.
        // 5.1 clears the whole window as PUC 5.1 does (its compiler drops a
        // leading `local x` LoadNil on that promise).
        let kept = nargs.saturating_sub(n_varargs).min(nparams);
        let window = if self.version == LuaVersion::Lua51 {
            max
        } else {
            nparams
        };
        let end = (base + window) as usize;
        // SAFETY: `need <= stack.len()` (resized above) and `base + kept <=
        // end <= need` since `kept <= nparams <= max_stack`.
        unsafe {
            self.stack
                .get_unchecked_mut((base + kept) as usize..end)
                .fill(Value::Nil)
        };
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Lua(Frame {
                closure: cl,
                base,
                pc: 0,
                func_slot,
                nresults,
                hook_oldpc: u32::MAX,
                from_c,
                n_varargs,
                // single-shot consume: `close_slots` sets pending_tm before each
                // handler call; the next Lua frame born is that handler's.
                tm: self.pending_tm.take(),
                // `run_hook` sets `pending_is_hook` before dispatching the user
                // hook so its frame reports `namewhat = "hook"` via getinfo.
                is_hook: std::mem::take(&mut self.pending_is_hook),
                tailcalls: std::mem::take(&mut self.pending_tailcalls),
                ccmt: std::mem::take(&mut self.pending_ccmt),
            }),
        );
        // PUC 5.1 `LUAI_COMPAT_VARARG`: the hidden `arg` local (the slot at
        // `base + nparams`) gets `{ n = n_varargs, e1, e2, … }` from the extras
        // just below `base` (5.1 db.lua :279 reads `arg.n` from a line hook).
        if proto.has_compat_vararg_arg {
            let arg_slot = (base + nparams) as usize;
            let t = self.heap.new_table();
            {
                // SAFETY: `t` was allocated above and is held only by this local; `tm` is the only reference into it, and the heap calls made while it lives (`set_int`, `intern`, `set`) do not collect
                let tm = unsafe { t.as_mut() };
                for i in 0..n_varargs {
                    let v = self.stack[(base - n_varargs + i) as usize];
                    // bounded by `n_varargs` (≤ MAXUPVAL territory), well
                    // below `MAX_ASIZE`
                    let _ = tm.set_int(&mut self.heap, (i + 1) as i64, v);
                }
                let nk = Value::Str(self.heap.intern(b"n"));
                tm.set(&mut self.heap, nk, Value::Int(n_varargs as i64))
                    .expect("'n' key");
            }
            // once-per-table barrier mirrors SETLIST: t is born BLACK during
            // Propagate and the bulk `set_int`/`set` calls above don't barrier
            self.heap
                .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
            self.stack[arg_slot] = Value::Table(t);
        }
        // PUC luaD_precall fires the "call" hook with the new frame current, so
        // a hook calling debug.getinfo(2) sees the entered function. For a Lua
        // callee, PUC `luaD_hookcall` passes `p->numparams` as ntransfer (only
        // fixed params count — extras already live below `base`).
        // A frame born via OP_TailCall fires "tail call" instead (PUC
        // luaD_pretailcall) and skips the matching "return" hook on exit.
        let is_tail = self
            .frames
            .last()
            .and_then(|f| f.lua())
            .is_some_and(|f| f.tailcalls > 0);
        self.hook_call_with(false, nparams, is_tail)?;
        Ok(())
    }
}
