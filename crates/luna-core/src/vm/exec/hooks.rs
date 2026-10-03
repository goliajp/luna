//! Debug hooks: the hook state, installing a hook, and firing the call,
//! return, line and count events.

use super::*;

/// Per-thread debug hook state (PUC `lua_State` hook/hookmask/basehookcount/
/// hookcount). `func` is the Lua hook; the booleans are the PUC mask bits.
#[derive(Clone, Copy, Default)]
pub struct HookState {
    /// the hook function (`None` when no hook is installed)
    pub func: Option<Value>,
    /// Rust-side debug hook. Fires alongside the Lua hook
    /// (Rust first); both can be installed simultaneously, but most
    /// embedders pick one.
    pub rust_func: Option<RustDebugHook>,
    /// LUA_MASKCALL — fire on function entry
    pub call: bool,
    /// LUA_MASKRET — fire on function return
    pub ret: bool,
    /// LUA_MASKLINE — fire on source-line change
    pub line: bool,
    /// LUA_MASKCOUNT — fire every `count_base` instructions
    pub count: bool,
    /// instruction count between count events (PUC basehookcount)
    pub count_base: i64,
    /// instructions left until the next count event (PUC hookcount)
    pub count_left: i64,
}

/// Rust-side debug hook callback. Receives the `Vm` plus a
/// classified event. The callback runs synchronously in the
/// dispatcher; the hook flag (`in_hook`) is set for its duration so
/// hook recursion is suppressed.
pub type RustDebugHook = fn(&mut Vm, RustHookEvent);

/// Classified debug event delivered to a [`RustDebugHook`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RustHookEvent {
    /// Function entry (`hook_call` analogue).
    Call,
    /// Function return (`hook_return` analogue).
    Return,
    /// Tail call entry (PUC 5.2+ separates this from a plain Call).
    TailCall,
    /// Source-line change (the `u32` is the 1-based line number).
    Line(u32),
    /// Instruction count event (fires every `count_base` instructions).
    Count,
}

/// Mask flags for [`Vm::set_rust_debug_hook`]. OR these to subscribe
/// to multiple event categories with a single hook installation.
pub const HOOK_MASK_CALL: u32 = 1;
/// Subscribe to function-return events.
pub const HOOK_MASK_RETURN: u32 = 2;
/// Subscribe to line-change events.
pub const HOOK_MASK_LINE: u32 = 4;
/// Subscribe to instruction-count events.
pub const HOOK_MASK_COUNT: u32 = 8;

impl Vm {
    /// Install or clear the debug hook on the running thread (`debug.sethook`
    /// without a thread argument). Arms the calling frame's `oldpc` to the
    /// sethook CALL's own pc (one less than the next-to-execute pc), mirroring
    /// PUC `rethook`'s `L->oldpc = pcRel(savedpc, p)` (= savedpc - code - 1) on
    /// native return: the very next traceexec compares against the sethook
    /// CALL's line. When the install statement and the following statement are
    /// on different source lines (db.lua :322), `changedline` fires for that
    /// first statement; when they share a line (db.lua :25 wrapper), they do
    /// not, so the wrapper line is not re-fired.
    pub(crate) fn install_hook(&mut self, hook: HookState) {
        self.hook = hook;
        self.trap = true;
        if self.hook.line
            && let Some(f) = self.frames.last_mut().and_then(CallFrame::lua_mut)
        {
            f.hook_oldpc = f.pc.saturating_sub(1);
        }
    }

    /// Install a hook on `target` (`None`/current thread → the live VM fields;
    /// another, suspended thread → its saved `Coro` state). PUC `debug.sethook`
    /// with an optional thread argument.
    ///
    /// `target == None` means "no explicit thread argument" — PUC binds that
    /// to `L` (the running thread). luna's live VM fields (`self.hook`,
    /// `self.frames`, `self.stack`) ARE the running thread's state, regardless
    /// of whether that's the main thread or a currently-resumed coroutine
    /// (save/restore happens at resume/yield boundaries via `load_coro_ctx`/
    /// `store_coro_ctx`). So a `None` target should always route to
    /// `install_hook` on the live fields. The pre-fix predicate gate
    /// `is_current_thread(target)` returned `false` when running inside a
    /// coroutine (`self.current = Some(co)`, `target = None` don't match)
    /// and silently dropped the hook on the floor — the install happened on
    /// no thread at all.
    pub(crate) fn set_hook(&mut self, target: Option<Gc<Coro>>, state: HookState) {
        if target.is_none() || self.is_current_thread(target) {
            self.install_hook(state);
        } else if let Some(co) = target {
            // SAFETY: `co` is a thread the caller holds (a native argument) and not the running one, so the Vm holds no reference into its saved frames; `m` is the only reference into it until the function returns
            let m = unsafe { co.as_mut() };
            m.hook = state;
            if state.line
                && let Some(f) = m.frames.last_mut().and_then(CallFrame::lua_mut)
            {
                f.hook_oldpc = u32::MAX;
            }
            // co.hook.func is a traced Value (Coro::trace covers it); demote
            // co back to gray so propagate sees the new hook function.
            self.heap.barrier_back(co);
        }
    }

    /// The hook state of `target` (`None`/current → the live VM state).
    pub(crate) fn get_hook(&self, target: Option<Gc<Coro>>) -> HookState {
        match target {
            t if self.is_current_thread(t) => self.hook,
            Some(co) => co.hook,
            None => self.hook,
        }
    }

    /// Invoke the debug hook for `event` (PUC `luaD_hook`). The hook runs with
    /// hooks disabled (PUC clears the mask) and its results/stack growth are
    /// discarded so the interrupted frame's register window is untouched.
    /// `line` is the source line for a "line" event, `None` (→ nil) otherwise.
    pub(super) fn run_hook(
        &mut self,
        event: &[u8],
        line: Option<i64>,
        from_native: bool,
    ) -> Result<(), LuaError> {
        // line and count events transfer no values (PUC `luaD_hook(L,
        // event, line, 0, 0)`); call and return hooks set theirs first
        if matches!(event, b"line" | b"count") {
            self.hook_ftransfer = 0;
            self.hook_ntransfer = 0;
        }
        // Rust hook fires first (no Vm reentrancy via call_value;
        // synchronous fn pointer call). Both Rust and Lua hooks may be
        // installed; both observe each event.
        if let Some(rh) = self.hook.rust_func {
            let evt = match event {
                b"call" => Some(RustHookEvent::Call),
                b"return" => Some(RustHookEvent::Return),
                b"tail call" | b"tail return" => Some(RustHookEvent::TailCall),
                b"line" => Some(RustHookEvent::Line(line.unwrap_or(0).max(0) as u32)),
                b"count" => Some(RustHookEvent::Count),
                _ => None,
            };
            if let Some(evt) = evt {
                let was_in_hook = self.in_hook;
                self.in_hook = true;
                // PUC `luaD_hook` roots the whole running frame while a hook
                // runs: a register written after the last safe point may sit
                // above `gc_top`, and the hook may collect
                let gc_top = self.gc_top;
                self.gc_top = gc_top.max(self.stack.len() as u32);
                rh(self, evt);
                self.gc_top = gc_top;
                self.in_hook = was_in_hook;
                self.trap = true;
            }
        }
        let Some(hook) = self.hook.func else {
            return Ok(());
        };
        if let (Value::LightUserdata(cf), Some(host)) = (hook, self.host_hook) {
            return self.run_host_hook(host, cf, event, line);
        }
        let saved_top = self.top;
        let saved_len = self.stack.len();
        let name = Value::Str(self.heap.intern(event));
        let lv = line.map_or(Value::Nil, Value::Int);
        self.in_hook = true;
        // PUC `db_sethook`'s C trampoline `hookf` sits between the engine and
        // the Lua hook — so `getinfo(2)` inside the hook resolves to whatever
        // ci sat below `hookf` (the function being hooked). When that hooked
        // function is native, no Lua frame for it exists in luna's `frames`;
        // model it as a synthetic C level by pushing the hook with
        // `from_c = true` (then `c_frame_name` reads the caller's call
        // instruction → e.g. `name = "sethook"`). When the hooked function is
        // Lua (its frame is still on the stack), push with `from_c = false`
        // so the level descent lands on it directly. The hook's own frame
        // carries `is_hook = true` so `getinfo(1).namewhat` reports "hook"
        // (PUC `CIST_HOOKED`).
        self.pending_is_hook = true;
        let r = self.call_value_impl(hook, &[name, lv], from_native);
        self.pending_is_hook = false;
        self.in_hook = false;
        self.trap = true;
        self.stack.truncate(saved_len);
        self.top = saved_top;
        r.map(|_| ())
    }

    /// Run the thread's C hook `cf` through the C API's dispatcher, as
    /// `run_hook` runs a Lua hook: with hooks off, and the whole running
    /// frame rooted.
    fn run_host_hook(
        &mut self,
        host: super::host_c::HostHookFn,
        cf: *const (),
        event: &[u8],
        line: Option<i64>,
    ) -> Result<(), LuaError> {
        self.in_hook = true;
        let gc_top = self.gc_top;
        self.gc_top = gc_top.max(self.stack.len() as u32);
        let saved_top = self.top;
        let r = host(self, cf, event, line);
        self.gc_top = gc_top;
        self.top = saved_top;
        self.in_hook = false;
        self.trap = true;
        r
    }

    /// Fire the "call" hook on entry to a function, if armed and not already in
    /// a hook (PUC clears the mask while a hook runs). PUC's transferinfo for
    /// a call hook is the param window: ftransfer = 1, ntransfer = nargs.
    /// `is_tail` selects the "tail call" event (PUC `LUA_HOOKTAILCALL`); a
    /// tail-call hook has no matching return hook (PUC luaD_pretailcall).
    pub(super) fn hook_call_with(
        &mut self,
        from_native: bool,
        nargs: u32,
        is_tail: bool,
    ) -> Result<(), LuaError> {
        if self.hook.call
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            self.hook_ftransfer = 1;
            self.hook_ntransfer = nargs.min(u16::MAX as u32) as u16;
            // PUC 5.1 didn't distinguish tail-call events — every call,
            // including tail-calls, fired plain `"call"`. 5.2 introduced
            // the separate `"tail call"` event (mask `"c"` covers both).
            // 5.1 db.lua :366 pins this with `{"call","call","call","call",
            // "return","tail return","return","tail return"}`.
            let event: &[u8] = if is_tail && self.version >= LuaVersion::Lua52 {
                b"tail call"
            } else {
                b"call"
            };
            self.run_hook(event, None, from_native)?;
        }
        Ok(())
    }

    pub(crate) fn hook_call(&mut self, from_native: bool, nargs: u32) -> Result<(), LuaError> {
        self.hook_call_with(from_native, nargs, false)
    }

    /// Fire the "return" hook on exit from a function, if armed. ftransfer is
    /// the first result slot relative to the activation's func slot, ntransfer
    /// the number of results.
    pub(crate) fn hook_return(
        &mut self,
        from_native: bool,
        ftransfer: u32,
        nresults: u32,
    ) -> Result<(), LuaError> {
        if self.hook.ret
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            self.hook_ftransfer = ftransfer.min(u16::MAX as u32) as u16;
            self.hook_ntransfer = nresults.min(u16::MAX as u32) as u16;
            self.run_hook(b"return", None, from_native)?;
        }
        Ok(())
    }

    /// PUC "tail return" event — fires once per tail call that collapsed
    /// into the activation now returning, *after* its own "return" event.
    /// 5.1 hook mask `"r"` covers both `return` and `tail return`.
    pub(super) fn hook_tail_return(&mut self) -> Result<(), LuaError> {
        if self.hook.ret
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            self.run_hook(b"tail return", None, false)?;
        }
        Ok(())
    }

    /// A count or line hook fires on the next instruction.
    #[inline]
    pub(super) fn hook_armed(&self) -> bool {
        !self.in_hook && (self.hook.func.is_some() || self.hook.rust_func.is_some())
    }

    /// Count and line hooks (PUC `traceexec`), fired before the instruction
    /// at `pc` of `cl` runs; `oldpc` is where the frame's line hook last looked.
    #[inline(never)]
    pub(super) fn exec_hooks(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        oldpc: u32,
    ) -> Result<(), LuaError> {
        let lines = &cl.proto.lines;
        // count hook: fire every `count_base` instructions
        let mut counthook = false;
        if self.hook.count {
            self.hook.count_left -= 1;
            if self.hook.count_left <= 0 {
                self.hook.count_left = self.hook.count_base;
                counthook = true;
            }
        }
        // the instruction a hook yielded at: its hooks have run (5.2+)
        if self.hook_resumed && (counthook || self.hook.line) {
            self.hook_resumed = false;
            return Ok(());
        }
        if counthook {
            // hooked function is the running Lua frame: its frame
            // is on the stack, so no synthetic C level is needed.
            // a count event carries no line (PUC `luaD_hook(L,
            // LUA_HOOKCOUNT, -1)`): the Lua hook gets nil
            self.run_hook(b"count", None, false)?;
        }
        // line hook: fire on a fresh frame, a backward jump (loop), or a
        // change of source line.
        if self.hook.line {
            if lines.is_empty() {
                // PUC: a stripped chunk has no line info, so
                // `getfuncline` returns -1. The line hook still fires
                // on the first instruction of the new frame (where
                // `npci <= oldpc` holds at oldpc=0), with the line
                // pushed as `nil` instead of an integer (db.lua :1030
                // "hook called without debug info for 1st instruction").
                if oldpc == u32::MAX {
                    self.run_hook(b"line", None, false)?;
                    self.top_frame_mut().hook_oldpc = pc;
                }
            } else {
                let newline = lines[(pc as usize).min(lines.len() - 1)];
                // PUC `traceexec`: fire on frame entry (`oldpc == MAX`),
                // on a backward jump (`pc < oldpc` — strict; an equal pc
                // would re-fire the install-site after `oldpc = pc`),
                // or when the source line changes.
                let fire = oldpc == u32::MAX
                    || pc < oldpc
                    || newline != lines[(oldpc as usize).min(lines.len() - 1)];
                if fire {
                    self.run_hook(b"line", Some(newline as i64), false)?;
                }
                self.top_frame_mut().hook_oldpc = pc;
            }
        }
        if std::mem::take(&mut self.hook_yield) {
            return Err(self.yield_from_hook(pc, counthook));
        }
        Ok(())
    }

    /// A C line or count hook yielded before the instruction at `pc` ran
    /// (PUC `luaG_traceexec`'s `L->status == LUA_YIELD`): suspend the
    /// coroutine with no values; the resume runs the instruction. 5.2+
    /// mark the instruction so its hooks are not called again, and put the
    /// count back one short of the event; 5.1 just runs it again.
    fn yield_from_hook(&mut self, pc: u32, counthook: bool) -> LuaError {
        let v51 = self.version == LuaVersion::Lua51;
        if counthook && !v51 {
            self.hook.count_left = 1;
        }
        let f = self.top_frame_mut();
        f.pc = pc;
        if v51 {
            // PUC 5.1 leaves `savedpc` at the instruction, so the line hook
            // compares with the line of the one before it
            f.hook_oldpc = pc.checked_sub(1).unwrap_or(u32::MAX);
        }
        self.yielding = Some((Vec::new(), HOOK_YIELD_SLOT, 0));
        LuaError(Value::Nil)
    }
}

/// The resume point of a coroutine a hook suspended: no call waits for the
/// resume's values.
pub(crate) const HOOK_YIELD_SLOT: u32 = u32::MAX;
