//! Queuing and running `__gc` finalizers, and the incremental step.

use super::stack_size::{self, Grown};
use super::*;

impl Vm {
    /// Register a table for finalization if its (just-set) metatable carries a
    /// `__gc` metamethod (PUC luaC_checkfinalizer at setmetatable time — adding
    /// `__gc` to the metatable afterwards does not retroactively register).
    pub(crate) fn check_finalizer(&mut self, t: Gc<Table>) -> Result<(), LuaError> {
        // Tables gained finalizers in 5.2; PUC 5.1 runs `__gc` for userdata only.
        if self.version == crate::version::LuaVersion::Lua51 {
            return Ok(());
        }
        if !self.get_mm(Value::Table(t), Mm::Gc).is_nil() {
            // `t` is the operand of the running `setmetatable`, a root
            self.retry_oom(|vm| vm.heap.register_finalizable(t))?;
        }
        Ok(())
    }

    /// Same as [`Self::check_finalizer`] for a userdata (the C API's
    /// `lua_setmetatable` from 5.2 on). PUC 5.1 instead marks every
    /// userdata that gets a metatable and looks for `__gc` when it is
    /// collected, as `newproxy` does.
    pub(crate) fn check_finalizer_userdata(
        &mut self,
        u: Gc<crate::runtime::Userdata>,
    ) -> Result<(), LuaError> {
        if !self.get_mm(Value::Userdata(u), Mm::Gc).is_nil() {
            self.retry_oom(|vm| vm.heap.register_finalizable_userdata(u))?;
        }
        Ok(())
    }

    /// Every finalizer, as the state closes (`luaC_freeallobjects` calls
    /// them with no `luaD_checkminstack`); errors go nowhere.
    pub(super) fn run_finalizers_closing(&mut self) {
        if self.gc_finalizing {
            return;
        }
        self.heap.queue_all_finalizers();
        while self.heap.has_tobefnz() && self.finalizer_has_room(true) {
            let _ = self.gc_tm(false);
        }
    }

    /// The finalizers a full collection calls once it has swept (5.2's
    /// all, after its pause is set; the others as their steps call them,
    /// before the pause), and the pause it ends with.
    pub(super) fn full_gc_finalizers(&mut self) -> Result<(), LuaError> {
        if self.version == LuaVersion::Lua52 {
            return self.call_all_pending(true);
        }
        self.gc_steps_to_pause()?;
        self.gc_full_repause();
        Ok(())
    }

    /// 5.2 `callallpendingfinalizers`: every pending finalizer.
    pub(super) fn call_all_pending(&mut self, propagate: bool) -> Result<(), LuaError> {
        while self.heap.has_tobefnz() {
            self.gc_tm(propagate)?;
        }
        Ok(())
    }

    /// PUC `GCTM`: call the first pending finalizer, with hooks off and no
    /// collection step while it runs (5.1: a threshold twice the count;
    /// later: the collector not running). With `propagate`, the error of
    /// a 5.1–5.3 finalizer is the error of the caller (5.1 raises it from
    /// inside the finalizer, under the running message handler); a 5.4 or
    /// 5.5 one is a warning.
    pub(super) fn gc_tm(&mut self, propagate: bool) -> Result<(), LuaError> {
        let propagate = propagate && !self.fin_quiet;
        let Some(obj) = self.heap.take_one_tobefnz() else {
            return Ok(());
        };
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("f"));
        let d = self.version;
        let in_hook = std::mem::replace(&mut self.in_hook, true);
        let finalizing = std::mem::replace(&mut self.gc_finalizing, true);
        let gc_top = self.gc_top;
        let stopped = self.heap.gc_is_stopped();
        let fin_stop = self.heap.gc_fin_stop;
        let threshold = self.heap.pace.threshold();
        match d {
            LuaVersion::Lua51 => {
                let t = 2 * self.heap.bytes() as i64;
                self.heap.pace.set_threshold(t);
            }
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.heap.gc_restore_stopped(true),
            _ => self.heap.gc_fin_stop = true,
        }
        self.heap.refresh_gc_limit();
        let r = self.run_one_finalizer(obj, propagate);
        self.in_hook = in_hook;
        self.gc_finalizing = finalizing;
        self.gc_top = gc_top;
        match d {
            // a 5.1 error leaves the threshold the finalizer ran under
            LuaVersion::Lua51 => {
                if r.is_ok() {
                    self.heap.pace.set_threshold(threshold);
                    self.heap.gc_restore_stopped(stopped);
                }
            }
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.heap.gc_restore_stopped(stopped),
            _ => self.heap.gc_fin_stop = fin_stop,
        }
        self.heap.refresh_gc_limit();
        r
    }

    /// 5.4/5.5 `luaD_checkminstack`: a finalizer runs only with two C levels
    /// to spare, two `CallInfo`s past the running one and `BASIC_STACK_SIZE`
    /// free slots, made when missing; otherwise it waits for a later
    /// collection.
    pub(super) fn finalizer_has_room(&mut self, closing: bool) -> bool {
        if self.version < LuaVersion::Lua54 {
            return true;
        }
        if self.g.nccalls + 2 >= super::MAX_C_DEPTH {
            return false;
        }
        if closing {
            return true;
        }
        self.extend_ci_to(self.ci_in_use() + 2);
        let top = self.native_top().unwrap_or(self.gc_top);
        let s = self.stack_size;
        // PUC's top is luna's plus one (`stack_size`)
        if top + stack_size::BASIC < stack_size::last(self.version, s) {
            return true;
        }
        match stack_size::grow(self.version, s, top, stack_size::BASIC) {
            Grown::Fits(n) => {
                self.regrow_stack(true, n);
                true
            }
            Grown::Overflow(n) => {
                self.resize_stack(n);
                false
            }
            Grown::ErrErr => false,
        }
    }

    /// Call `obj`'s `__gc`. With `propagate`, the error a 5.1–5.3
    /// finalizer raised comes back as the error the collection raises (5.1
    /// calls it unprotected); 5.4+ turn it into a warning.
    fn run_one_finalizer(&mut self, obj: Value, propagate: bool) -> Result<(), LuaError> {
        let gc = self.get_mm(obj, Mm::Gc);
        // 5.2/5.3 call only a function (`__gc = true` just marks the object
        // for finalization); 5.1 and 5.4+ call any value
        let call = match self.version {
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                matches!(gc, Value::Closure(_) | Value::Native(_))
            }
            _ => !gc.is_nil(),
        };
        if !call {
            return Ok(());
        }
        // a 5.4+ `__gc` that cannot be called warns as calling it would fail
        // in GCTM, with no position (the running level is not Lua code)
        if self.version >= LuaVersion::Lua54
            && !matches!(gc, Value::Closure(_) | Value::Native(_))
            && self.get_mm(gc, Mm::Call).is_nil()
        {
            let tn = self.obj_typename(gc);
            let msg = format!("attempt to call a {tn} value (metamethod '__gc')");
            let v = match self.alloc_str(msg.as_bytes()) {
                Ok(s) => Value::Str(s),
                Err(e) => e.0,
            };
            self.warn_error("__gc", v);
            return Ok(());
        }
        // the frame born from this call is named `__gc` (PUC `CIST_FIN`);
        // saved around the call in case the handler is a native, which never
        // takes it
        let saved_tm = self
            .pending_tm
            .replace(crate::runtime::function::FrameTm::Gc);
        // PUC `GCTM` pushes the finalizer at `L->top`, above everything in
        // use; a native's top can lie below results a call it made has just
        // returned (print's nested tostring)
        let (len, top) = (self.stack.len(), self.top);
        let at = self.native_top().map(|t| t.max(top));
        let r = if propagate && self.version == LuaVersion::Lua51 {
            // `luaD_call`: an error leaves through the finalizer, under the
            // message handler that is running; it cannot yield
            self.nny += 1;
            let r = self.call_value_impl(gc, &[obj], false, at);
            self.nny -= 1;
            r
        } else {
            match at {
                Some(at) => self.call_protected_at(gc, &[obj], at),
                None => self.call_protected(gc, &[obj]),
            }
        };
        // the call gives back the slots it took, not the window of a native
        // below it, and leaves the stack and its top as they were (`GCTM`)
        if self.stack.len() < len {
            self.stack.regrow(len, Value::Nil);
        } else {
            self.stack.truncate(len);
        }
        self.top = top;
        self.pending_tm = saved_tm;
        let Err(e) = r else {
            return Ok(());
        };
        // `luaD_pcall` shrinks the stack it caught the error on
        if self.version >= LuaVersion::Lua52 {
            self.shrink_caught_error();
        }
        if self.version >= LuaVersion::Lua54 {
            self.warn_error("__gc", e.0);
            return Ok(());
        }
        if !propagate {
            return Ok(());
        }
        if self.version == LuaVersion::Lua51 {
            return Err(e);
        }
        Err(self.gcmm_error(e.0))
    }

    /// 5.2/5.3 `LUA_ERRGCMM`: "error in __gc metamethod (msg)", thrown past
    /// any message handler.
    fn gcmm_error(&mut self, inner: Value) -> LuaError {
        self.gcmm_raised += 1;
        let msg = match inner {
            Value::Str(s) => s.as_bytes().to_vec(),
            _ => b"no message".to_vec(),
        };
        // made piece by piece, as `luaO_pushfstring` makes it
        let fmt = b"error in __gc metamethod (%s)";
        let v = match self.pushfstring(fmt, &[crate::runtime::puc_fmt::FmtArg::S(&msg)]) {
            Ok(s) => Value::Str(s),
            Err(e) => e.0,
        };
        self.handler_bypass = Some(v);
        LuaError(v)
    }
}
