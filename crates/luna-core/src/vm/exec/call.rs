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
                    self.check_lua_stack(func_slot + 1 + nargs, 20, false)?;
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
                    self.running_natives
                        // a tail call resolved its `__call` chain before
                        // calling here and passed the count in tail_ccmt
                        .push_or_abort(crate::vm::callstack::NativeAct::new(
                            nc,
                            func_slot,
                            nargs,
                            self.frames.len(),
                            tail_ccmt + chain as u8,
                        ));
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
                    chain += 1;
                    self.shift_in_call_mm(v, func_slot, nargs, chain)?;
                    nargs += 1;
                }
            }
        }
    }

    /// One `__call` hop of [`Vm::begin_call`] (PUC tryfuncTM): put the
    /// handler of `v` below the callee and its arguments. Out of line: it is
    /// rare, and keeping it inside makes the common call path pay for the
    /// registers it needs.
    #[cold]
    #[inline(never)]
    fn shift_in_call_mm(
        &mut self,
        v: Value,
        func_slot: u32,
        nargs: u32,
        chain: u32,
    ) -> Result<(), LuaError> {
        let mm = self.get_mm(v, Mm::Call);
        if mm.is_nil() || self.call_mm_unusable(mm) {
            return Err(self.call_err_at(v, func_slot + 1 + nargs));
        }
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
            self.grow_stack_or_abort(end + 1);
        }
        self.stack.copy_within(from..end, from + 1);
        self.stack[from] = mm;
        if self.top > func_slot {
            self.top += 1;
        }
        Ok(())
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
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            self.grow_frames()?;
        }
        let proto = cl.proto;
        self.check_lua_stack(
            func_slot + 1 + nargs,
            proto.max_stack as u32,
            proto.is_vararg,
        )?;
        let nparams = proto.num_params as u32;
        // 5.5 vararg layout (PUC luaT_adjustvarargs): the extra args stay on the
        // stack just below the new `base`, so a named vararg can be indexed
        // virtually without allocating a table. Rotate `[p1..pn][e1..em]` to
        // `[e1..em][p1..pn]` so the fixed params land at the new base.
        let n_varargs = nargs.saturating_sub(nparams) * u32::from(proto.is_vararg);
        // a vararg frame sits where PUC puts it, so a recursion through
        // vararg functions takes the same stack: above the arguments, where
        // 5.1 to 5.3 copy the fixed parameters (`adjust_varargs`) and 5.4 on
        // the function too (`luaT_adjustvarargs`); the extras stay just
        // below the base, the slots they came from are dead
        let gap = if proto.is_vararg {
            nparams + u32::from(self.version >= LuaVersion::Lua54)
        } else {
            0
        };
        let base = func_slot + 1 + n_varargs + gap;
        if proto.is_vararg && nargs > 0 {
            let s = (func_slot + 1) as usize;
            let kept = nargs.min(nparams) as usize;
            let end = (base + kept as u32) as usize;
            if self.stack.len() < end {
                self.grow_stack_or_abort(end);
            }
            self.stack.copy_within(s..s + kept, base as usize);
            if n_varargs > 0 {
                let from = s + nparams as usize;
                let to = (base - n_varargs) as usize;
                self.stack.copy_within(from..from + n_varargs as usize, to);
            }
        }
        let max = proto.max_stack as u32;
        let need = (base + max) as usize;
        if self.stack.len() < need {
            self.grow_stack_or_abort(need);
        }
        // Only missing parameters become nil (PUC `luaD_precall`): the code
        // writes the rest before reading it, the collector keeps it valid
        // (`clear_dead_stack`) and a trace checks only what it reads first.
        // 5.1 clears the whole window as PUC 5.1 does (its compiler drops a
        // leading `local x` LoadNil on that promise).
        let kept = nargs.saturating_sub(n_varargs).min(nparams);
        // 5.5's `luaT_adjustvarargs` sets the vararg parameter, the
        // register after the fixed ones, to nil
        let window = if self.version == LuaVersion::Lua51 {
            max
        } else if self.version == LuaVersion::Lua55 && proto.is_vararg {
            (nparams + 1).min(max)
        } else {
            nparams
        };
        let end = (base + window) as usize;
        // SAFETY: `need <= stack.len()` (resized above) and `base + kept <=
        // end <= need` since `kept <= nparams <= window <= max_stack`.
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
                // `luaH_new(L, nvar, 1)`: an array part of exactly the
                // extras' count (bounded by the stack, well below
                // `MAX_ASIZE`), nil ones included
                tm.resize(&mut self.heap, n_varargs as usize, 1);
                for i in 0..n_varargs {
                    tm.set_list_slot(i as usize, self.stack[(base - n_varargs + i) as usize]);
                }
                let nk = Value::Str(self.heap.intern(b"n"));
                tm.set(&mut self.heap, nk, Value::Int(n_varargs as i64))
                    .expect("'n' key");
            }
            // once-per-table barrier mirrors SETLIST: t is born BLACK during
            // Propagate and the bulk `set_int`/`set` calls above don't barrier
            self.heap.barrier_back(t);
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

    /// Room for `need` slots above `top` on the thread's stack (PUC
    /// `luaD_checkstack` at a call, `top` being `L->top` there), else the
    /// error PUC raises. `vararg`: a vararg function is called, which 5.4
    /// on checks one slot more for, as its frame moves up past the
    /// function and the arguments (`luaT_adjustvarargs`); the fast path
    /// counts that slot for every call and the slow one takes it back.
    #[inline(always)]
    pub(super) fn check_lua_stack(
        &mut self,
        top: u32,
        need: u32,
        vararg: bool,
    ) -> Result<(), LuaError> {
        // under the lowest dialect's limit by a slot to spare, no exact
        // count is needed (the constant keeps the fast path free of loads)
        if top + need < STACK_LIMIT_FLOOR {
            return Ok(());
        }
        let need = need + u32::from(vararg && self.version >= LuaVersion::Lua54);
        if top + need <= self.g.lua_stack_limit {
            return Ok(());
        }
        self.lua_stack_overflow(top, need)
    }

    /// PUC `luaD_growstack` past the limit: the first overflow raises
    /// "stack overflow" and opens `STACK_ERR_SPACE` more slots for the
    /// message handler that runs on it; a call that does not fit those
    /// either is "error in error handling" (errors.lua :606, cstack.lua
    /// :29). The space closes when a protected call catches the error.
    #[cold]
    #[inline(never)]
    fn lua_stack_overflow(&mut self, top: u32, need: u32) -> Result<(), LuaError> {
        if !self.stack_extra {
            self.stack_extra = true;
            // before 5.4 `luaG_runerror` adds the position to the message
            // of a Lua function, pushing it too; a C function (one whose
            // message handler is being called on a full stack) gets none
            let positioned = self.version() < LuaVersion::Lua54 && !self.native_on_top();
            self.overflow_top = Some(top + u32::from(positioned));
            return Err(self.rt_err("stack overflow"));
        }
        if top + need >= self.g.lua_stack_limit + STACK_ERR_SPACE {
            return Err(LuaError(self.errerr()));
        }
        Ok(())
    }

    /// Room for one more frame. 5.1 checks its call limit here, as PUC
    /// 5.1's `luaD_growCI` does when its `CallInfo` array is full: an array
    /// already past `LUAI_MAXCALLS` (grown for a message handler) is "error
    /// in error handling"; otherwise it doubles, and when that takes it
    /// past the limit the call raises "stack overflow". The frame array's
    /// capacity plays the `CallInfo` array's size; calls compiled code made
    /// natively (`frames_native`) count as frames too.
    #[cold]
    #[inline(never)]
    fn grow_frames(&mut self) -> Result<(), LuaError> {
        if self.g.frame_size > self.frame_cap {
            return Err(LuaError(self.errerr()));
        }
        self.g.frame_size *= 2;
        if self.g.frame_size > self.frame_cap {
            return Err(self.rt_err("stack overflow"));
        }
        Ok(())
    }

    /// The frames PUC 5.1 has in its `CallInfo` array for the running
    /// thread: the Lua frames and the protected calls on the frame stack,
    /// the native functions running on the Rust stack, and the calls
    /// compiled code made natively.
    pub(super) fn frames_in_use(&self) -> u32 {
        let natives = (self.running_natives.len() - self.natives_base) as u32;
        // a coroutine's array starts with its base frame; the main thread's
        // is the host's call, with the host's own frames below it
        let base = if self.current.is_some() {
            1
        } else {
            self.g.host_frames
        };
        self.frames.len() as u32 + natives + self.g.frames_native - self.g.meta_conts + base
    }

    /// PUC 5.1 `restore_stack_limit`, after a protected call caught an
    /// error: a frame array grown past the limit for a message handler goes
    /// back to the limit, unless the frames still in use come near it.
    pub(super) fn restore_frame_limit(&mut self) {
        if self.g.frame_size > self.frame_cap && self.frames_in_use() + 1 < self.frame_cap {
            self.g.frame_size = self.frame_cap;
        }
    }
}
