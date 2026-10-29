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
            Err(payload) => {
                // whatever window it came out of is closed now
                self.native_window_panic = 0;
                Err(self.native_panic_error(payload))
            }
        };
        self.native_returned(result)
    }

    /// The end of a native's run: an error pops it and notes it as the
    /// error's raiser.
    #[inline(always)]
    fn native_returned(&mut self, result: Result<u32, LuaError>) -> Result<u32, LuaError> {
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
                // a yield on its way out is no error of the native's
                if !self.control_in_flight() {
                    self.note_errored_native(act, e.0);
                }
                Err(e)
            }
        }
    }

    /// The Lua error a native's panic becomes, `native panic: <msg>`. A
    /// load's memory failure belongs to that load, never to a native it
    /// may sit under: it goes on unwinding.
    #[cold]
    #[inline(never)]
    pub(super) fn native_panic_error(
        &mut self,
        payload: Box<dyn std::any::Any + Send>,
    ) -> LuaError {
        if crate::runtime::mem::is_load_oom(&*payload) {
            std::panic::resume_unwind(payload)
        }
        let msg = panic_payload_str(&payload);
        match self.fmt_str(format_args!("native panic: {msg}")) {
            Ok(s) => LuaError(Value::Str(s)),
            Err(e) => e,
        }
    }

    /// A panic that unwound out of [`Vm::run`] to `exec_with`, which
    /// entered it with `natives_at_entry` natives running. The natives the
    /// fast loop calls ([`Vm::call_native_plain`]) have no guard of their
    /// own, so this does for them what [`Vm::invoke_native`] does: a panic
    /// that came out of a native this run called is that native's error.
    /// Any other goes on unwinding: one from the run's own code (no native
    /// of this run on the stack), from a hook of this run's native
    /// ([`Vm::in_native_window`]), or a load's memory failure.
    ///
    /// Between the native and here only `call_native_plain`, `run_fast`
    /// and `run` unwind, which on an error do nothing but return it: the
    /// error then leaves the state the guarded call would. A native this
    /// run called calls Lua at a level of its own, whose `exec_with` deals
    /// with what happens there; a panic that still comes out went through
    /// the native as it would have gone through its guard.
    #[cold]
    #[inline(never)]
    pub(super) fn unguarded_native_panic(
        &mut self,
        payload: Box<dyn std::any::Any + Send>,
        natives_at_entry: usize,
    ) -> LuaError {
        let window = self.native_window_panic as usize;
        if self.running_natives.len() == natives_at_entry || window == natives_at_entry + 1 {
            std::panic::resume_unwind(payload)
        }
        self.native_window_panic = 0;
        let e = self.native_panic_error(payload);
        // as `native_returned` pops: the top one, which is this run's
        // native or what a level below it left there
        let act = self.running_natives.pop().expect("checked above");
        self.note_errored_native(act, e.0);
        e
    }

    /// Run `f` with the native on top of `running_natives` not running
    /// (its call or return hook, the argument check of `pcall`, a C
    /// continuation's resume): a panic out of `f` is not the native's, so
    /// [`Vm::unguarded_native_panic`] must let it go on unwinding.
    pub(super) fn in_native_window<R>(&mut self, f: impl FnOnce(&mut Vm) -> R) -> R {
        let at = self.running_natives.len() as u32;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(r) => r,
            Err(payload) => {
                self.native_window_panic = at;
                std::panic::resume_unwind(payload)
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
        // what comes before and after the call are functions of their own:
        // in an unoptimised build their locals are then off the stack while
        // the native runs, which a chain of resumes repeats on every level
        self.enter_native_plain(nc, func_slot, nargs, nresults)?;
        // no guard here: a panic unwinds to `exec_with`, which raises it as
        // this native's error (see `Vm::unguarded_native_panic`)
        let result = (nc.f)(self, func_slot, nargs);
        self.leave_native_plain(func_slot, nargs, nresults, result)
    }

    /// `call_native_plain` up to the call.
    #[cfg_attr(luna_hot, inline(always))]
    fn enter_native_plain(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<(), LuaError> {
        // a C function gets `LUA_MINSTACK` slots (PUC `luaD_precall`)
        self.check_lua_stack(func_slot + 1 + nargs, 20, true)?;
        self.pending_tailcalls = 0;
        let ccmt = std::mem::take(&mut self.pending_ccmt);
        self.native_nresults = nresults;
        // the caller's registers sit below `func_slot`; the native's own
        // arguments stay rooted too (see `begin_call`)
        self.gc_top = func_slot + nargs + 1;
        self.push_native_fast(crate::vm::callstack::NativeAct::new(
            nc,
            func_slot,
            nargs,
            self.frames.len(),
            ccmt,
        ))?;
        self.precall_checkpoint(func_slot + 1 + nargs)
    }

    /// `call_native_plain` once the native returned `result`.
    #[cfg_attr(luna_hot, inline(always))]
    fn leave_native_plain(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        result: Result<u32, LuaError>,
    ) -> Result<(), LuaError> {
        let nret = self.native_returned(result)?;
        // the native may have armed a hook, whose return event it gets
        self.finish_native_call(func_slot, nargs, nret, nresults)
    }

    /// The native on top of `running_natives` returned `nret` results at
    /// `func_slot`: fire the return hook, pop it, adjust the results and
    /// give the collector its chance.
    #[cfg_attr(luna_hot, inline(always))]
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
            self.grow_stack(need)?;
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
        // the native's level given back; a bound made stale while it ran
        // stays far below any frame count
        self.g.ci_hint += 1;
        // no check for a collection: PUC's `luaD_poscall` makes none, the
        // native checked where its API calls do
        self.finish_results(func_slot, nret, nresults);
        Ok(())
    }
}
