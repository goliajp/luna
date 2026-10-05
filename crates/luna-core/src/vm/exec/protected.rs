//! Natives that run their callee under a continuation: pcall, xpcall
//! and pairs.

use super::*;

impl Vm {
    /// `pcall(f, ...)` (PUC luaB_pcall): push a continuation frame, then drive
    /// the protected call `f` through the interpreter loop. The protected
    /// function and its arguments already sit at `func_slot+1..`, so calling `f`
    /// at `func_slot+1` lets its results land one slot above the continuation —
    /// the loop head then writes `true` at `func_slot` to form `true, results…`.
    /// Always returns `Ok(true)`: a continuation is now on the stack to be
    /// resolved by the loop (even when `f` is a native that already ran inline).
    pub(super) fn begin_pcall(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<bool, LuaError> {
        if nargs == 0 {
            // `luaL_checkany` fails here: there is no function to call.
            self.with_native_running(func_slot, nargs, |vm| {
                let a = crate::vm::argcheck::Args::new(func_slot, nargs);
                crate::vm::argcheck::check_any(vm, a, 0).map(drop)
            })?;
        }
        if self.pcall_depth >= MAX_C_DEPTH {
            // raised inside pcall, a C function: no position
            return Err(self.plain_err("C stack overflow"));
        }
        self.pcall_depth += 1;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pcall,
                func_slot,
                nresults,
            }),
        );
        // call f (slot func_slot+1) with the remaining args, asking for all
        // results; a yield or error inside propagates with the continuation kept
        // on the stack (caught by `unwind` / preserved across a yield).
        self.begin_call(func_slot + 1, Some(nargs - 1), -1, true)?;
        Ok(true)
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
    ) -> Result<bool, LuaError> {
        if !host {
            self.with_native_running(func_slot, nargs, |vm| {
                let a = crate::vm::argcheck::Args::new(func_slot, nargs);
                crate::vm::builtins::xpcall_handler(vm, a).map(drop)
            })?;
        }
        if self.pcall_depth >= MAX_C_DEPTH {
            // raised inside pcall, a C function: no position
            return Err(self.plain_err("C stack overflow"));
        }
        self.pcall_depth += 1;
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
        self.top = func_slot + 2 + nfargs;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Xpcall { handler },
                func_slot,
                nresults,
            }),
        );
        self.begin_call(func_slot + 1, Some(nfargs), -1, true)?;
        Ok(true)
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
        self.running_natives.push(crate::vm::callstack::NativeAct {
            nc,
            func_slot,
            nargs,
            depth: self.frames.len() as u32,
            ccmt: 0,
        });
        let r = check(self);
        self.running_natives.pop();
        r
    }

    pub(super) fn begin_pairs(&mut self, func_slot: u32, nresults: i32) -> Result<bool, LuaError> {
        let arg = self.stack[(func_slot + 1) as usize];
        let mm = self.get_mm(arg, Mm::Pairs);
        // layout becomes [pairs@func_slot, mm@func_slot+1, t@func_slot+2]:
        // `pairs` keeps its slot so the debug interface can report it as the
        // C function running below the metamethod. Call mm(t) wanting 4.
        let need = (func_slot + 3) as usize;
        if self.stack.len() < need {
            self.grow_stack_or_abort(need);
        }
        self.stack[(func_slot + 2) as usize] = arg;
        self.stack[(func_slot + 1) as usize] = mm;
        self.top = func_slot + 3;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pairs,
                func_slot,
                nresults,
            }),
        );
        let want = crate::vm::builtins::pairs_mm_results(self) as i32;
        self.begin_call(func_slot + 1, Some(1), want, true)?;
        Ok(true)
    }
}
