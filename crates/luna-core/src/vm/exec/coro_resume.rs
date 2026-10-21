//! Switching between coroutines: saving and loading the running
//! context, resume and yield.

use super::*;

impl Vm {
    /// `coroutine.resume` core: drive `co` with `args` until it yields, returns
    /// or errors. Ok(values) carries yielded or returned values; Err carries an
    /// error raised inside the coroutine (the coroutine becomes dead).
    pub(crate) fn resume_coro(
        &mut self,
        co: Gc<Coro>,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, LuaError> {
        self.host_before_resume(co);
        match co.status {
            CoroStatus::Suspended => {}
            CoroStatus::Dead => return Err(self.plain_err("cannot resume dead coroutine")),
            _ => return Err(self.plain_err("cannot resume non-suspended coroutine")),
        }
        self.resume_c_level(co)?;
        // the coroutine runs from the resumer's count, plus the resume
        let nccalls = self.g.nccalls;
        self.g.nccalls += 1;
        let special_before = self.special_errors();
        let resumer = self.current;
        // save the resumer's live context away
        let rctx = self.take_ctx();
        match resumer {
            Some(r) => {
                // SAFETY: `r` is `self.current`, the running coroutine and so a root; no reference into it is live here, and `m` ends before the barrier call
                let m = unsafe { r.as_mut() };
                m.stack = rctx.stack;
                m.frames = rctx.frames;
                m.open_upvals = rctx.open_upvals;
                m.tbc = rctx.tbc;
                m.top = rctx.top;
                m.meta_conts = rctx.meta_conts;
                m.stale_frames = rctx.stale_frames;
                m.stack_size = rctx.stack_size;
                m.frame_size = rctx.frame_size;
                m.ci_alloc = rctx.ci_alloc;
                m.globals = rctx.globals;
                m.status = CoroStatus::Normal;
                m.natives = self.natives_base..self.running_natives.len();
                // bulk overwrite of every traced field on r — mirror
                // store_coro_ctx's barrier_back so propagate re-traces r.
                self.heap.barrier_back(r);
            }
            None => self.main_ctx = Some(rctx),
        }
        self.load_coro_ctx(co);
        // the continuations it was suspended in hold no level now
        self.g.stale_frames = self.frames.len() as u32;
        {
            // SAFETY: `co` is the argument being resumed, held by the caller (a stack slot or native argument) and about to become `self.current`; `load_coro_ctx`'s borrow has ended, so `m` is the only one
            let m = unsafe { co.as_mut() };
            m.status = CoroStatus::Running;
            m.resumer = resumer;
        }
        // co.resumer is a traced Gc field; barrier_back covers the new
        // resumer reference and any future field writes during this call.
        self.heap.barrier_back(co);
        self.current = Some(co);
        let resumer_natives_base = self.natives_base;
        self.natives_base = self.running_natives.len();
        // the coroutine's own frames start a fresh reach for xpcall handlers
        let resumer_msgh_floor = std::mem::replace(&mut self.msgh_floor, 0);
        let resumer_msgh_running = self.msgh_running.take();
        // a coroutine that dies keeps its traceback for `debug.traceback(co)`
        let resumer_keeps_traceback = std::mem::replace(&mut self.keep_error_traceback, true);
        // non-yieldable calls belong to the thread that made them (PUC's
        // per-thread `nny`): a coroutine resumed from inside one, such as a
        // host's `lua_pcall`, can still yield
        let resumer_nny = std::mem::replace(&mut self.nny, 0);

        // drive it
        let drive = if co.started {
            self.coro_continue(&args)
        } else {
            // SAFETY: `co` is `self.current`, a root; the temporary borrow covers one field store and nothing else refers into the coroutine
            unsafe { co.as_mut() }.started = true;
            self.coro_first(co.body, &args)
        };

        // classify: a self-close termination or a pending yield each win over
        // the (sentinel) error they raised to unwind the Rust stack.
        let (outcome, status) = if let Some(death) = self.terminating.take() {
            // the coroutine closed itself: it dies now, cleanly or with the
            // error a `__close` handler raised.
            let r = match death {
                Some(e) => {
                    self.set_coro_error(co, e, crate::runtime::ErrorStatus::Run);
                    (Err(LuaError(e)), CoroStatus::Dead)
                }
                None => (Ok(Vec::new()), CoroStatus::Dead),
            };
            self.host_thread_reset(co);
            r
        } else {
            match self.yielding.take() {
                Some((vals, fslot, nres)) => {
                    // SAFETY: `co` is still `self.current`, a root; the borrow covers two field stores
                    let c = unsafe { co.as_mut() };
                    c.resume_at = Some((fslot, nres));
                    c.yield_ci_top = std::mem::take(&mut self.yield_ci_top);
                    (Ok(vals), CoroStatus::Suspended)
                }
                None => {
                    // died: a return is clean, an error is remembered so a later
                    // `coroutine.close` can report it (PUC lua_closethread).
                    // The frames taken where the error was raised stay with
                    // the dead coroutine, as PUC's untouched dead stack does
                    // (db.lua :848 family)
                    if drive.is_err() {
                        let dead = self.dead_snapshot.take();
                        // SAFETY: `co` is still `self.current`, a root, and its frames have unwound, so `m` is the only reference into it for this store
                        unsafe { co.as_mut() }.dead = dead;
                        self.heap.barrier_back(co);
                    }
                    if let Err(e) = drive {
                        let kind = self.error_status(e.0, special_before);
                        self.set_coro_error(co, e.0, kind);
                    }
                    (self.host_returned(co, drive), CoroStatus::Dead)
                }
            }
        };

        // save the coroutine's context back and restore the resumer
        self.natives_base = resumer_natives_base;
        self.msgh_floor = resumer_msgh_floor;
        self.msgh_running = resumer_msgh_running;
        self.keep_error_traceback = resumer_keeps_traceback;
        self.nny = resumer_nny;
        self.store_coro_ctx(co);
        self.set_coro_status(co, status);
        match resumer {
            Some(r) => {
                self.load_coro_ctx(r);
                self.set_coro_status(r, CoroStatus::Running);
                self.current = Some(r);
            }
            None => {
                let m = self.main_ctx.take().expect("main context saved");
                self.put_ctx(m);
                self.current = None;
            }
        }
        self.g.nccalls = nccalls;
        outcome
    }

    /// First resume: install the body function at slot 0 and run.
    pub(super) fn coro_first(
        &mut self,
        body: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, LuaError> {
        self.stack.clear();
        self.grow_stack(1 + args.len())?;
        self.stack[0] = body;
        self.stack[1..].copy_from_slice(args);
        self.top = self.stack.len() as u32;
        let r = self.begin_call(0, Some(args.len() as u32), -1, true);
        // the body is the call's function now, held by nothing else: it
        // dies when the call is gone (PUC keeps it only on the stack)
        if let Some(co) = self.current {
            // SAFETY: `co` is `self.current`, a root; one field store
            unsafe { co.as_mut() }.body = Value::Nil;
        }
        match r {
            Ok(true) => self.exec_with(1),
            Ok(false) => Ok(self.take_results(0)),
            Err(e) => Err(e),
        }
    }

    /// Resume after a yield: deliver `args` as the results of the call that
    /// yielded, then continue the suspended thread.
    pub(super) fn coro_continue(&mut self, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        let (fslot, nres) = self.current.unwrap().resume_at.expect("resume point");
        let n = args.len() as u32;
        // Restore the full register window of the suspended top frame: a yield
        // that unwound through a native (call_value) may have left the stack
        // shorter than the frame needs. `base + max_stack` is what push_frame
        // allocates; `fslot + n` covers the delivered yield results.
        let frame_need = self
            .frames
            .last()
            .and_then(CallFrame::lua)
            .map(|f| (f.base + f.closure.proto.max_stack as u32) as usize)
            .unwrap_or(0);
        if fslot == HOOK_YIELD_SLOT {
            // a hook yielded: the instruction it interrupted runs now
            self.grow_stack(frame_need)?;
            self.hook_resumed = self.version >= LuaVersion::Lua52;
            return self.exec_with(1);
        }
        // the `coroutine.yield` returning, for a C hook's return event
        let yielder = match self.stack.get(fslot as usize) {
            Some(&Value::Native(nc)) if self.c_hook_installed() => Some(nc),
            _ => None,
        };
        let need = frame_need.max((fslot + n) as usize);
        self.grow_stack(need)?;
        for (i, &v) in args.iter().enumerate() {
            self.stack[fslot as usize + i] = v;
        }
        self.finish_results(fslot, n, nres);
        // the suspended `coroutine.yield` (a C call) now returns its resume
        // values: fire the matching "return" hook PUC defers until the resume.
        // A C function that yielded with a continuation returns only once
        // its continuation has.
        let host_cont = matches!(
            self.frames.last(),
            Some(CallFrame::Cont(nc)) if matches!(nc.kind, ContKind::Host(_))
        );
        if !host_cont {
            // the yield is a level of its own while its return hook runs
            if let Some(nc) = yielder {
                self.push_native(crate::vm::callstack::NativeAct::new(
                    nc,
                    fslot,
                    0,
                    self.frames.len(),
                    0,
                ))?;
            }
            let r = self.hook_return(true, 1, n);
            if yielder.is_some() {
                self.running_natives.pop();
            }
            r?;
        }
        self.exec_with(1)
    }

    /// The C function yielding, on top of the running natives.
    pub(crate) fn note_yield_ci_top(&mut self) {
        self.yield_ci_top = self.running_natives.last().map_or(0, |a| a.ci_top);
    }

    /// `coroutine.yield`: suspend the running coroutine, recording where to
    /// resume. Errors if called outside a coroutine. Returns a sentinel error
    /// that `exec`/`resume_coro` recognise as a yield (never surfaced to Lua).
    pub(crate) fn do_yield(&mut self, func_slot: u32, vals: Vec<Value>) -> LuaError {
        self.note_yield_ci_top();
        let nres = self.native_nresults;
        self.yielding = Some((vals, func_slot, nres));
        // value is irrelevant: resume_coro consults `self.yielding`, not this
        LuaError(Value::Nil)
    }

    /// `co`, which the caller holds, takes `status`.
    pub(crate) fn set_coro_status(&mut self, co: Gc<Coro>, status: CoroStatus) {
        // SAFETY: a thread's fields are only written through the Vm that
        // runs it, which this `&mut self` is; its callers hold the thread as
        // a `Gc` handle and no reference into it, so this borrow, which
        // covers one store, is the only one
        unsafe { co.as_mut() }.status = status;
    }

    /// `co`, which the caller holds, died with `err`, raised as `kind`:
    /// what `coroutine.close` and the C API's status report for it.
    pub(crate) fn set_coro_error(
        &mut self,
        co: Gc<Coro>,
        err: Value,
        kind: crate::runtime::ErrorStatus,
    ) {
        // SAFETY: as in `set_coro_status`: the only reference into the
        // thread, for two stores
        let m = unsafe { co.as_mut() };
        m.error_value = Some(err);
        m.error_status = kind;
        self.heap.barrier_back(co);
    }
}
