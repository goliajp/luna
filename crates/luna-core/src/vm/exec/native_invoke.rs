//! Running a native function from a call site.

use super::*;

impl Vm {
    /// Run the native on top of `running_natives`, popping it on an error.
    /// A Rust panic in the native surfaces as a Lua error rather than
    /// unwinding into the embedder, which should then drop the Vm: its
    /// state may be inconsistent. `AssertUnwindSafe` is sound because any
    /// half-done state is fenced behind the `Err` returned.
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
            // a load's memory failure belongs to that load, never to a
            // native it may sit under
            Err(payload) if crate::runtime::mem::is_load_oom(&*payload) => {
                std::panic::resume_unwind(payload)
            }
            Err(payload) => {
                let msg = panic_payload_str(&payload);
                let s = Value::Str(self.heap.intern(format!("native panic: {msg}").as_bytes()));
                Err(LuaError(s))
            }
        };
        match result {
            Ok(n) => {
                // a native runs Lua through `call_value`, which raises
                // `nny` so `yield_barrier` refuses a yield below it, or
                // through `call_value_k`, whose yield it passes on as `Err`
                debug_assert!(self.yielding.is_none(), "a native returned over a yield");
                Ok(n)
            }
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
        // a C function gets `LUA_MINSTACK` slots (PUC `luaD_precall`)
        self.check_lua_stack(func_slot + 1 + nargs, 20, false)?;
        self.pending_tailcalls = 0;
        let ccmt = std::mem::take(&mut self.pending_ccmt);
        self.native_nresults = nresults;
        // the caller's registers sit below `func_slot`; the native's own
        // arguments stay rooted too (see `begin_call`)
        self.gc_top = func_slot + nargs + 1;
        self.running_natives
            .push_or_abort(crate::vm::callstack::NativeAct::new(
                nc,
                func_slot,
                nargs,
                self.frames.len(),
                ccmt,
            ));
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
            && !std::mem::take(&mut self.native_ret_hooked)
        {
            let res_dst = func_slot + nargs + 1;
            let need = (res_dst + nret) as usize;
            if self.stack.len() < need {
                self.grow_stack_or_abort(need);
            }
            for i in (0..nret).rev() {
                self.stack[(res_dst + i) as usize] = self.stack[(func_slot + i) as usize];
            }
            // widen the C-frame's argument window for getlocal, and its top
            // past the results: the hook runs above them (PUC `rethook`)
            let saved_off = self.running_natives.last().map_or(0, |act| act.top_off);
            if let Some(act) = self.running_natives.last_mut() {
                act.nargs = nargs + nret;
                act.top_off = 0;
            }
            let hr = self.hook_return(true, nargs + 1, nret);
            if let Some(act) = self.running_natives.last_mut() {
                act.nargs = nargs;
                act.top_off = saved_off;
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
