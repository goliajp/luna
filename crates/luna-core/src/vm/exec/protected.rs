//! Natives that run their callee under a continuation: pcall, xpcall
//! and pairs.

use super::*;

/// Who makes a protected call (see `Vm::begin_pcall`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtectedBy {
    /// `pcall` / `xpcall` called from Lua
    Lua,
    /// the C API's `lua_pcall`
    Host,
    /// lua.c's `docall`, from inside `pmain`
    HostInC,
}

impl Vm {
    /// `pcall(f, ...)` (PUC luaB_pcall): push a continuation frame, then drive
    /// the protected call `f` through the interpreter loop. The protected
    /// function and its arguments already sit at `func_slot+1..`, so calling `f`
    /// at `func_slot+1` lets its results land one slot above the continuation —
    /// the loop head then writes `true` at `func_slot` to form `true, results…`.
    /// Always returns `Ok(true)`: a continuation is now on the stack to be
    /// resolved by the loop (even when `f` is a native that already ran inline).
    /// `by`: who makes the call. `pcall` from Lua makes its call through
    /// `lua_pcallk`, and lua.c's `docall` through `lua_pcall` inside
    /// `pmain`: each a C level of PUC's; a host's `lua_pcall` is the level
    /// its own call took (`call_value`), no more. The callee is called
    /// where PUC calls it (`callee_shift`), so a recursion through
    /// protected calls takes the same stack.
    pub(super) fn begin_pcall(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        by: ProtectedBy,
    ) -> Result<bool, LuaError> {
        if nargs == 0 {
            // `luaL_checkany` fails here: there is no function to call.
            self.with_native_running(func_slot, nargs, |vm| {
                let a = crate::vm::argcheck::Args::new(func_slot, nargs);
                crate::vm::argcheck::check_any(vm, a, 0).map(drop)
            })?;
        }
        let level = by != ProtectedBy::Host;
        self.enter_pcall_level(level)?;
        let shift = self.callee_shift(by, false);
        let callee = self.shift_callee(func_slot + 1, nargs, shift);
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pcall { level, shift },
                func_slot,
                nresults,
            }),
        );
        // call f with the remaining args, asking for all results; a yield or
        // error inside propagates with the continuation kept on the stack
        // (caught by `unwind` / preserved across a yield).
        self.begin_call(callee, Some(nargs - 1), -1, true)?;
        Ok(true)
    }

    /// Where PUC's `pcall` / `xpcall` call their function, counted from
    /// the slot above their own: the status result they push first (5.2
    /// on), the handler `xpcall` keeps below (5.1, 5.2) or the function it
    /// copies above them both (5.3 on). A host's `lua_pcall` calls the
    /// function where the host pushed it, which the native that drives the
    /// call sits on (-1); lua.c's `docall` has its handler there.
    fn callee_shift(&self, by: ProtectedBy, xpcall: bool) -> i8 {
        match (by, xpcall) {
            (ProtectedBy::Host, _) => -1,
            (ProtectedBy::HostInC, _) => 0,
            (ProtectedBy::Lua, false) => i8::from(self.version >= LuaVersion::Lua52),
            (ProtectedBy::Lua, true) if self.version >= LuaVersion::Lua53 => 3,
            (ProtectedBy::Lua, true) => 1,
        }
    }

    /// Move the callee and its `n - 1` arguments at `from` to `from +
    /// shift`, where it is called; the slots they pass over belong to the
    /// protected call itself.
    fn shift_callee(&mut self, from: u32, n: u32, shift: i8) -> u32 {
        let to = (i64::from(from) + i64::from(shift)) as u32;
        let end = (to + n) as usize;
        if self.stack.len() < end {
            self.grow_stack_or_abort(end);
        }
        if shift != 0 {
            self.stack
                .copy_within(from as usize..(from + n) as usize, to as usize);
        }
        to
    }

    /// `xpcall(f, msgh, ...)` (PUC luaB_xpcall): like `begin_pcall`, but the
    /// message handler is stashed in the continuation and the arguments are
    /// shifted down over the handler's slot so `f`'s args are contiguous.
    /// `forward` is false for 5.1's `xpcall`, which passes `f` none of them.
    /// A host's protected call (`host`) takes any value as the handler, as
    /// `lua_pcall` does: one that cannot be called fails only when an error
    /// needs it.
    pub(super) fn begin_xpcall(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        forward: bool,
        host: bool,
        by: ProtectedBy,
    ) -> Result<bool, LuaError> {
        if !host {
            self.with_native_running(func_slot, nargs, |vm| {
                let a = crate::vm::argcheck::Args::new(func_slot, nargs);
                crate::vm::builtins::xpcall_handler(vm, a).map(drop)
            })?;
        }
        let level = by != ProtectedBy::Host;
        self.enter_pcall_level(level)?;
        // layout: [xpcall@func_slot, f@+1, msgh@+2, a1@+3, ...]. Stash msgh and
        // close its gap so f's args become [f@+1, a1@+2, ...].
        let handler = self.stack[(func_slot + 2) as usize];
        // 5.1: `xpcall (f, err)` takes exactly two parameters — extra
        // arguments are NOT forwarded to `f` (5.2 added forwarding;
        // 5.1 calls f with zero args).
        let nfargs = if forward { nargs - 2 } else { 0 };
        for i in 0..nfargs {
            self.stack[(func_slot + 2 + i) as usize] = self.stack[(func_slot + 3 + i) as usize];
        }
        let shift = self.callee_shift(by, true);
        let callee = self.shift_callee(func_slot + 1, nfargs + 1, shift);
        self.top = callee + 1 + nfargs;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Xpcall {
                    handler,
                    level,
                    shift,
                },
                func_slot,
                nresults,
            }),
        );
        self.begin_call(callee, Some(nfargs), -1, true)?;
        Ok(true)
    }

    /// The continuation of a protected call takes a C level when it is one
    /// of PUC's (see `begin_pcall`); the error is raised inside pcall, a C
    /// function: no position.
    fn enter_pcall_level(&mut self, level: bool) -> Result<(), LuaError> {
        if level {
            self.check_c_level(false)?;
            self.g.nccalls += 1;
        }
        Ok(())
    }

    /// `pairs(t)` where `t` has a `__pairs` metamethod (PUC luaB_pairs's
    /// lua_callk path): drive `__pairs(t)` through the loop with a `Pairs`
    /// continuation so a `coroutine.yield` inside it suspends cleanly. The
    /// metamethod is called in `pairs`'s own slot, so its (≤4, nil-padded)
    /// results land exactly where `pairs`'s results belong.
    /// Run a check of the native at `func_slot` while it counts as the running
    /// C function, so an argument error names it the way PUC does. pcall and
    /// xpcall check their arguments in the dispatcher, before the native
    /// would otherwise be entered.
    pub(super) fn with_native_running(
        &mut self,
        func_slot: u32,
        nargs: u32,
        check: impl FnOnce(&mut Vm) -> Result<(), LuaError>,
    ) -> Result<(), LuaError> {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("pcall/xpcall dispatch sits on a native")
        };
        self.running_natives
            .push_or_abort(crate::vm::callstack::NativeAct::new(
                nc,
                func_slot,
                nargs,
                self.frames.len(),
                0,
            ));
        let r = check(self);
        let act = self.running_natives.pop().expect("pushed above");
        // raised by that native, as it would have been once entered
        if let Err(e) = &r {
            self.note_errored_native(act, e.0);
        }
        r
    }

    pub(super) fn begin_pairs(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<bool, LuaError> {
        self.enter_c_level(false)?;
        let arg = self.stack[(func_slot + 1) as usize];
        let mm = self.get_mm(arg, Mm::Pairs);
        // mm(t) is pushed above pairs's arguments and called there, wanting
        // 4; `pairs` keeps its slot so the debug interface can report it as
        // the C function running below the metamethod
        let at = 1 + nargs;
        let need = (func_slot + at + 2) as usize;
        if self.stack.len() < need {
            self.grow_stack_or_abort(need);
        }
        self.stack[(func_slot + at + 1) as usize] = arg;
        self.stack[(func_slot + at) as usize] = mm;
        self.top = func_slot + at + 2;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pairs { at },
                func_slot,
                nresults,
            }),
        );
        let want = crate::vm::builtins::pairs_mm_results(self) as i32;
        self.begin_call(func_slot + at, Some(1), want, true)?;
        Ok(true)
    }

    /// Put `f` and `args` at `slot` for a call made there (see
    /// `call_value_impl`), over whatever the stack holds above it.
    #[cold]
    #[inline(never)]
    pub(super) fn place_call(&mut self, slot: u32, f: Value, args: &[Value]) -> u32 {
        let end = slot as usize + 1 + args.len();
        if self.stack.len() < end {
            self.grow_stack_or_abort(end);
        }
        self.stack[slot as usize] = f;
        self.stack[slot as usize + 1..end].copy_from_slice(args);
        slot
    }
}
