//! Switching between coroutines: saving and loading the running
//! context, resume and yield.

use super::*;
use crate::runtime::mem::LVec;

/// A thread's swapped-out execution context (PUC per-thread stack state).
pub(super) struct SavedCtx {
    pub(super) stack: LVec<Value>,
    pub(super) frames: LVec<CallFrame>,
    pub(super) open_upvals: LVec<(u32, Gc<Upvalue>)>,
    pub(super) tbc: LVec<u32>,
    pub(super) top: u32,
    pub(super) pcall_depth: u32,
    pub(super) hook: HookState,
    /// PUC `L->l_gt` — the thread's own globals table. Carried alongside
    /// the rest of the suspended state so each thread can keep its own
    /// `setfenv(0, env)` rewire without the swap leaking into another
    /// thread (5.1 closure.lua :177).
    pub(super) globals: Gc<Table>,
}

impl Vm {
    pub(super) fn take_ctx(&mut self) -> SavedCtx {
        let saved = SavedCtx {
            stack: self.stack.take(),
            frames: self.frames.take(),
            open_upvals: self.open_upvals.take(),
            tbc: self.tbc.take(),
            top: self.top,
            pcall_depth: self.pcall_depth,
            hook: self.hook,
            globals: self.globals,
        };
        self.frames_resync(); // frames now empty
        saved
    }

    pub(super) fn put_ctx(&mut self, c: SavedCtx) {
        self.stack = c.stack;
        self.frames = c.frames;
        self.open_upvals = c.open_upvals;
        self.tbc = c.tbc;
        self.top = c.top;
        self.pcall_depth = c.pcall_depth;
        self.hook = c.hook;
        self.globals = c.globals;
        self.frames_resync(); // sync shadow to new Vec
    }

    /// Move a coroutine's saved context into the live VM fields.
    pub(super) fn load_coro_ctx(&mut self, co: Gc<Coro>) {
        // SAFETY: `co` is the coroutine `resume_coro` is switching to (or its resumer `r`), which the caller holds and which is a root through `self.current` or a saved stack; `m` is the only reference into it until the function returns, and nothing here can collect
        let m = unsafe { co.as_mut() };
        self.stack = m.stack.take();
        self.frames = m.frames.take();
        self.open_upvals = m.open_upvals.take();
        self.tbc = m.tbc.take();
        self.top = m.top;
        self.frames_resync(); // sync shadow to coro's frames
        self.pcall_depth = m.pcall_depth;
        self.hook = m.hook;
        self.globals = m.globals;
    }

    /// Save the live VM context back into a coroutine object.
    pub(super) fn store_coro_ctx(&mut self, co: Gc<Coro>) {
        let c = self.take_ctx();
        // SAFETY: `co` is the coroutine `resume_coro` is switching away from, held by its caller; `take_ctx` above did not touch it, and `m` is the only reference into it until the barrier call, which takes only its address
        let m = unsafe { co.as_mut() };
        m.stack = c.stack;
        m.frames = c.frames;
        m.open_upvals = c.open_upvals;
        m.tbc = c.tbc;
        m.top = c.top;
        m.pcall_depth = c.pcall_depth;
        m.hook = c.hook;
        m.globals = c.globals;
        // bulk-overwrite of every collectable field traced by Coro::trace:
        // demote the coro back to gray so propagate re-traces its new state.
        self.heap.barrier_back(co);
    }

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
        if self.c_depth >= MAX_C_DEPTH || native_stack::is_low(native_stack::RESERVE) {
            return Err(self.plain_err("C stack overflow"));
        }
        self.c_depth += 1;
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
                m.pcall_depth = rctx.pcall_depth;
                m.globals = rctx.globals;
                m.status = CoroStatus::Normal;
                m.natives = self.natives_base..self.running_natives.len();
                // bulk overwrite of every traced field on r — mirror
                // store_coro_ctx's barrier_back so propagate re-traces r.
                self.heap.barrier_back(r);
            }
            None => self.main_ctx = Some(rctx),
        }
        // swap the coroutine in
        self.load_coro_ctx(co);
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
                    // SAFETY: `co` is still `self.current`, a root, and the coroutine's frames have all unwound; the borrow covers two field stores
                    let m = unsafe { co.as_mut() };
                    m.error_value = Some(e);
                    m.error_status = crate::runtime::ErrorStatus::Run;
                    self.heap.barrier_back(co);
                    (Err(LuaError(e)), CoroStatus::Dead)
                }
                None => (Ok(Vec::new()), CoroStatus::Dead),
            };
            self.host_thread_reset(co);
            r
        } else {
            match self.yielding.take() {
                Some((vals, fslot, nres)) => {
                    // SAFETY: `co` is still `self.current`, a root; the borrow covers one field store
                    unsafe { co.as_mut() }.resume_at = Some((fslot, nres));
                    (Ok(vals), CoroStatus::Suspended)
                }
                None => {
                    // died: a return is clean, an error is remembered so a later
                    // `coroutine.close` can report it (PUC lua_closethread).
                    // Keep the error-point traceback (taken by `unwind` before
                    // popping the failing frames) so `debug.traceback(co)` on
                    // the dead coroutine still shows the error site, as PUC's
                    // untouched dead stack does (db.lua :848 family).
                    if drive.is_err() {
                        let levels = self.error_traceback.take().unwrap_or_default();
                        let mut tb = b"stack traceback:".to_vec();
                        tb.extend(crate::vm::callstack::traceback_from_lines(
                            self.version,
                            &levels,
                            0,
                            0,
                        ));
                        let mem = self.heap.mem();
                        let mut kept = LVec::new(mem);
                        for l in &levels {
                            kept.push_or_abort(LVec::from_slice_or_abort(mem, l));
                        }
                        // SAFETY: `co` is still `self.current`, a root, and its frames have unwound, so `m` is the only reference into it for these two stores
                        let m = unsafe { co.as_mut() };
                        m.error_traceback = Some(LVec::from_slice_or_abort(mem, &tb));
                        m.error_levels = Some(kept);
                    }
                    if let Err(e) = drive {
                        let kind = self.error_status(e.0, special_before);
                        // SAFETY: `co` is still `self.current`, a root; the borrow covers two field stores
                        let m = unsafe { co.as_mut() };
                        m.error_value = Some(e.0);
                        m.error_status = kind;
                        self.heap.barrier_back(co);
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
        // SAFETY: `co` is still `self.current`, a root; `store_coro_ctx`'s borrow has ended, and this one covers one field store
        unsafe { co.as_mut() }.status = status;
        match resumer {
            Some(r) => {
                self.load_coro_ctx(r);
                // SAFETY: `r` is the resumer, made `self.current` again on the next line and held by the caller's frames meanwhile; `load_coro_ctx`'s borrow has ended, and this one covers one field store
                unsafe { r.as_mut() }.status = CoroStatus::Running;
                self.current = Some(r);
            }
            None => {
                let m = self.main_ctx.take().expect("main context saved");
                self.put_ctx(m);
                self.current = None;
            }
        }
        self.c_depth -= 1;
        outcome
    }

    /// First resume: install the body function at slot 0 and run.
    pub(super) fn coro_first(
        &mut self,
        body: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, LuaError> {
        self.stack.clear();
        self.stack.push_or_abort(body);
        self.stack.extend_from_slice_or_abort(args);
        self.top = self.stack.len() as u32;
        match self.begin_call(0, Some(args.len() as u32), -1, true) {
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
            if self.stack.len() < frame_need {
                self.grow_stack_or_abort(frame_need);
            }
            self.hook_resumed = self.version >= LuaVersion::Lua52;
            return self.exec_with(1);
        }
        // the `coroutine.yield` returning, for a C hook's return event
        let yielder = match self.stack.get(fslot as usize) {
            Some(&Value::Native(nc)) if self.c_hook_installed() => Some(nc),
            _ => None,
        };
        let need = frame_need.max((fslot + n) as usize);
        if self.stack.len() < need {
            self.grow_stack_or_abort(need);
        }
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
                self.running_natives
                    .push_or_abort(crate::vm::callstack::NativeAct {
                        nc,
                        func_slot: fslot,
                        nargs: 0,
                        depth: self.frames.len() as u32,
                        ccmt: 0,
                    });
            }
            let r = self.hook_return(true, 1, n);
            if yielder.is_some() {
                self.running_natives.pop();
            }
            r?;
        }
        self.exec_with(1)
    }

    /// `coroutine.yield`: suspend the running coroutine, recording where to
    /// resume. Errors if called outside a coroutine. Returns a sentinel error
    /// that `exec`/`resume_coro` recognise as a yield (never surfaced to Lua).
    pub(crate) fn do_yield(&mut self, func_slot: u32, vals: Vec<Value>) -> LuaError {
        let nres = self.native_nresults;
        self.yielding = Some((vals, func_slot, nres));
        // value is irrelevant: resume_coro consults `self.yielding`, not this
        LuaError(Value::Nil)
    }
}
