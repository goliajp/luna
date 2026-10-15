//! Queuing and running `__gc` finalizers, and the incremental step.

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

    /// Run pending `__gc` finalizers (objects the collector resurrected for
    /// finalization), from an automatic collection: errors do not reach the
    /// mutator. Reentrancy-guarded.
    pub(super) fn run_finalizers(&mut self) {
        let _ = self.run_finalizers_or_err(false);
    }

    /// Run the pending finalizers. With `propagate` (an explicit collection
    /// on 5.1–5.3), the first error stops the run and is returned; the
    /// finalizers after it stay queued for the next collection, as in PUC,
    /// where `GCTM` throws out of the loop that calls them.
    pub(super) fn run_finalizers_or_err(&mut self, propagate: bool) -> Result<(), LuaError> {
        if self.gc_finalizing {
            return Ok(());
        }
        let pending = self.heap.take_tobefnz();
        if pending.is_empty() {
            return Ok(());
        }
        self.gc_finalizing = true;
        // PUC `GCTM` turns hooks off while a finalizer runs (`allowhook`)
        let in_hook = std::mem::replace(&mut self.in_hook, true);
        let mut first_err: Option<LuaError> = None;
        let mut ran = 0;
        for &obj in &pending {
            if !self.finalizer_has_room() {
                break;
            }
            ran += 1;
            if let Err(e) = self.run_one_finalizer(obj)
                && propagate
            {
                first_err = Some(e);
                break;
            }
        }
        if ran < pending.len() {
            self.heap.requeue_tobefnz(&pending[ran..]);
        }
        self.in_hook = in_hook;
        self.gc_finalizing = false;
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// 5.4/5.5 `luaD_checkminstack`: a finalizer runs only with two C levels
    /// to spare; otherwise it waits for a later collection.
    fn finalizer_has_room(&self) -> bool {
        self.version < LuaVersion::Lua54 || self.g.nccalls + 2 < super::MAX_C_DEPTH
    }

    /// Call `obj`'s `__gc`. The error a 5.1–5.3 finalizer raised comes back
    /// as the error the collection raises; 5.4+ turn it into a warning.
    fn run_one_finalizer(&mut self, obj: Value) -> Result<(), LuaError> {
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
        let r = match self.native_top() {
            Some(t) => self.call_protected_at(gc, &[obj], t.max(top)),
            None => self.call_protected(gc, &[obj]),
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
        if self.version >= LuaVersion::Lua54 {
            self.warn_error("__gc", e.0);
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
        let mut text = b"error in __gc metamethod (".to_vec();
        text.extend_from_slice(&msg);
        text.push(b')');
        let v = match self.alloc_str(&text) {
            Ok(s) => Value::Str(s),
            Err(e) => e.0,
        };
        self.handler_bypass = Some(v);
        LuaError(v)
    }
}
