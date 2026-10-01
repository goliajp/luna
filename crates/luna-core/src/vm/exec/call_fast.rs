//! The Lua-to-Lua call the fast loop makes without `begin_call`.

use super::*;

impl Vm {
    /// Push the frame of a Lua function called from the fast loop (PUC
    /// `luaD_precall` for a Lua function) when the frame is all the call
    /// needs: no method JIT to try, a function with fixed parameters and
    /// the stack already big enough. The caller runs with no hook and no
    /// trace JIT, so neither has anything to see. The new frame, which the
    /// caller then runs; `None`, having done nothing, leaves the call to
    /// `begin_call`.
    #[inline(always)]
    pub(super) fn push_lua_frame_fast(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Option<*mut Frame> {
        let p = cl.proto;
        let base = func_slot + 1;
        let need = base as usize + p.max_stack as usize;
        if self.jit.enabled
            || p.is_vararg
            || p.has_compat_vararg_arg
            || func_slot + 256 > MAX_LUA_STACK
            || self.stack.len() < need
        {
            return None;
        }
        // as `push_frame`: missing parameters are nil, and 5.1 clears the
        // whole window
        let kept = nargs.min(p.num_params as u32);
        let end = if self.version == LuaVersion::Lua51 {
            need
        } else {
            base as usize + p.num_params as usize
        };
        // SAFETY: `need <= stack.len()` was checked above and `base + kept
        // <= end <= need` (the verifier keeps `num_params <= max_stack`)
        unsafe {
            self.stack
                .get_unchecked_mut((base + kept) as usize..end)
                .fill(Value::Nil);
        }
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
                from_c: false,
                n_varargs: 0,
                tm: self.pending_tm.take(),
                is_hook: std::mem::take(&mut self.pending_is_hook),
                tailcalls: std::mem::take(&mut self.pending_tailcalls),
                ccmt: std::mem::take(&mut self.pending_ccmt),
            }),
        );
        // SAFETY: a Lua frame was just pushed
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(f) => Some(f),
            // SAFETY: see above
            CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
        }
    }
}

/// What [`Vm::return_fast`] did.
pub(super) enum Returned {
    /// nothing: the loop head's `Return` arm takes over
    No,
    /// to a metamethod's continuation, which sets `trap`
    ToMeta,
    /// to this Lua frame, now on top
    ToLua(*mut Frame),
}

impl Vm {
    /// `Return0` / `Return1` without the close and hook machinery (PUC
    /// `OP_RETURN0` / `OP_RETURN1`): when no return hook can fire, nothing
    /// in this frame needs closing and the caller is a Lua frame or a
    /// metamethod's continuation inside this activation, the return is the
    /// pop, the move of the (at most one) result and the result count that
    /// `complete_return` would do. `false`, having done nothing, otherwise.
    /// Without `WATCH` no hook is armed: arming one sets `trap`, which the
    /// fast loop leaves for its head before running anything else.
    #[inline(always)]
    pub(super) fn return_fast<const WATCH: bool>(
        &mut self,
        base: u32,
        abs_a: u32,
        nret: u32,
        entry_depth: usize,
    ) -> Returned {
        let n = self.frames.len();
        if n <= entry_depth
            || n < 2
            || WATCH && self.hook.ret && self.hook_armed()
            || self.open_upvals.last().is_some_and(|&(s, _)| s >= base)
            || self.tbc.last().is_some_and(|&s| s >= base)
        {
            return Returned::No;
        }
        // SAFETY: `n >= 2`; popping the top frame leaves this one in place
        let caller: Option<*mut Frame> = match unsafe { self.frames.get_unchecked_mut(n - 2) } {
            CallFrame::Lua(f) => Some(f),
            CallFrame::Cont(c) if matches!(c.kind, ContKind::Meta(_)) => None,
            CallFrame::Cont(_) => return Returned::No,
        };
        let to_meta = caller.is_none();
        // SAFETY: the running frame is on top, and it is a Lua frame
        let (func_slot, wanted) = match unsafe { self.frames.get_unchecked(n - 1) } {
            CallFrame::Lua(f) => (f.func_slot, f.nresults),
            // SAFETY: see above
            CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
        };
        frames_pop_known(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            to_meta,
        );
        if nret == 1 {
            // SAFETY: both slots are in the returning frame's window or the
            // caller's, which the stack holds
            unsafe {
                let s = self.stack.as_mut_ptr();
                Value::copy_raw(s.add(func_slot as usize), s.add(abs_a as usize));
            }
        }
        if to_meta || wanted < 0 {
            self.top = func_slot + nret;
        } else {
            let w = wanted as u32;
            if nret < w {
                self.pad_results(func_slot + nret, func_slot + w);
            } else if nret > w {
                // one result, none wanted
                self.stack[func_slot as usize] = Value::Nil;
            }
            self.top = func_slot + w;
        }
        match caller {
            Some(f) => Returned::ToLua(f),
            None => Returned::ToMeta,
        }
    }
}

impl Vm {
    /// Run the native on top of `running_natives`, popping it on an error.
    /// A Rust panic in the native surfaces as a Lua error rather than
    /// unwinding through the VM into the embedder. The VM's state may still
    /// be inconsistent after a panic (half-pushed args, dangling GC
    /// references), so an embedder that catches this class of error should
    /// drop and re-create the Vm — but that beats tearing the host process
    /// down. `AssertUnwindSafe` is sound because the caller is the dispatch
    /// loop and any half-done state is fenced behind the `Err` returned.
    #[inline(always)]
    pub(super) fn invoke_native(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
    ) -> Result<u32, LuaError> {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let result = match catch_unwind(AssertUnwindSafe(|| (nc.f)(self, func_slot, nargs))) {
            Ok(r) => r,
            Err(payload) => {
                let msg = panic_payload_str(&payload);
                let s = Value::Str(self.heap.intern(format!("native panic: {msg}").as_bytes()));
                Err(LuaError(s))
            }
        };
        match result {
            Ok(n) => Ok(n),
            Err(e) => {
                // PUC raises with the native still on the stack; remember it
                // for the handler and traceback of the error (see
                // `raise_to_handler`)
                let act = self.running_natives.pop().expect("pushed by the caller");
                self.note_errored_native(act, e.0);
                Err(e)
            }
        }
    }

    /// A plain native called from the fast loop (PUC `precallC`): what
    /// `begin_call` does for one, without the call and return hooks, which
    /// the fast loop runs without, and staying in the calling frame.
    #[inline(never)]
    pub(super) fn call_native_plain(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<(), LuaError> {
        self.pending_tailcalls = 0;
        let ccmt = std::mem::take(&mut self.pending_ccmt);
        self.native_nresults = nresults;
        // the caller's registers sit below `func_slot`; the native's own
        // arguments stay rooted too (see `begin_call`)
        self.gc_top = func_slot + nargs + 1;
        self.running_natives.push(crate::vm::callstack::NativeAct {
            nc,
            func_slot,
            nargs,
            depth: self.frames.len() as u32,
            ccmt,
        });
        let nret = self.invoke_native(nc, func_slot, nargs)?;
        // the native may have armed a hook, whose return event it gets
        self.finish_native_call(func_slot, nargs, nret, nresults)
    }

    /// The native on top of `running_natives` returned `nret` results at
    /// `func_slot`: fire the return hook, pop it, adjust the results and
    /// give the collector its chance.
    #[inline(always)]
    pub(super) fn finish_native_call(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nret: u32,
        nresults: i32,
    ) -> Result<(), LuaError> {
        // PUC `luaD_poscall` fires the return hook BEFORE moving
        // results into the function's slot — at that point args
        // sit at `[func_slot + 1, func_slot + 1 + nargs)` and
        // results above them at `[func_slot + 1 + nargs, …)`.
        // luna's `nat_return` has already written the results
        // into `[func_slot, func_slot + nret)`, so we replay PUC's
        // layout by copying the results up past the preserved
        // args, firing the hook (with ftransfer = nargs + 1, so
        // `getlocal(2, ftransfer..)` reads results), and then
        // copying back for `finish_results`. db.lua :541 reads
        // `getinfo("r").ftransfer` + `getlocal` to inspect a
        // returning native's results this way.
        if self.hook.ret
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            let res_dst = func_slot + nargs + 1;
            let need = (res_dst + nret) as usize;
            if self.stack.len() < need {
                self.stack.resize(need, Value::Nil);
            }
            for i in (0..nret).rev() {
                self.stack[(res_dst + i) as usize] = self.stack[(func_slot + i) as usize];
            }
            // widen the C-frame's argument window for getlocal
            if let Some(act) = self.running_natives.last_mut() {
                act.nargs = nargs + nret;
            }
            let hr = self.hook_return(true, nargs + 1, nret);
            if let Some(act) = self.running_natives.last_mut() {
                act.nargs = nargs;
            }
            // restore results into the slot finish_results expects
            for i in 0..nret {
                self.stack[(func_slot + i) as usize] = self.stack[(res_dst + i) as usize];
            }
            self.running_natives.pop();
            hr?;
        } else {
            self.running_natives.pop();
        }
        self.finish_results(func_slot, nret, nresults);
        // the native may have allocated; collect with the results as
        // the live boundary (PUC checks GC after a call returns).
        self.maybe_collect_garbage(self.top);
        Ok(())
    }
}
