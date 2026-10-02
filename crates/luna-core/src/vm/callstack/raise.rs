//! Raising an error: its position, the natives it passed through, and
//! the handler that catches it.

use crate::runtime::Value;
use crate::runtime::function::{CallFrame, ContKind};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

use super::NativeAct;

/// A native that raised the error in flight, and where it ran.
#[derive(Clone, Copy)]
pub(crate) struct ErroredNative {
    act: NativeAct,
    err: Value,
}

impl Vm {
    /// PUC `luaG_runerror`: `msg` with the position of the running function
    /// when that is a Lua function; a running native adds none.
    pub(crate) fn runerror(&mut self, msg: &str) -> crate::vm::error::LuaError {
        if self.native_on_top() {
            self.plain_err(msg)
        } else {
            self.rt_err(msg)
        }
    }

    /// Is the running function a native (the level-0 `CallInfo` a C one)?
    pub(crate) fn native_on_top(&self) -> bool {
        self.running_natives.len() > self.natives_base
            && self
                .running_natives
                .last()
                .is_some_and(|a| a.depth as usize == self.frames.len())
    }

    /// Remember that `nc` raised `err`. A native only leaves the stack by
    /// the time the error reaches `unwind`, where PUC still has it; natives
    /// an error passes through on its way out collect innermost first, and
    /// a different error starts the list over.
    pub(crate) fn note_errored_native(&mut self, act: NativeAct, err: Value) {
        let continues = self
            .errored_natives
            .last()
            .is_some_and(|inner| inner.err.raw_eq(err) && inner.act.depth >= act.depth);
        if !continues {
            self.errored_natives.clear();
        }
        self.errored_natives.push(ErroredNative { act, err });
    }

    /// The natives recorded for `err` that were running at the top of the
    /// stack, innermost first.
    fn take_errored_natives(&mut self, err: Value) -> Vec<ErroredNative> {
        let mut list = std::mem::take(&mut self.errored_natives);
        let depth = self.frames.len() as u32;
        if !list
            .iter()
            .all(|e| e.err.raw_eq(err) && e.act.depth == depth)
        {
            list.clear();
        }
        list
    }

    /// The pcall (`Some(None)`) or xpcall (`Some(Some(handler))`) that will
    /// catch an error raised now, if any is in reach.
    fn nearest_catcher(&self) -> Option<Option<Value>> {
        let floor = self.msgh_floor.min(self.frames.len());
        self.frames[floor..]
            .iter()
            .rev()
            .find_map(|cf| match cf {
                CallFrame::Cont(nc) => match nc.kind {
                    ContKind::Pcall => Some(None),
                    ContKind::Xpcall { handler } => Some(Some(handler)),
                    _ => None,
                },
                CallFrame::Lua(_) => None,
            })
            // uncaught inside a running handler: that handler again
            .or(self.msgh_running.map(Some))
    }

    /// PUC `luaG_errormsg`, at the point the error reaches the unwinder:
    /// with the stack that raised it still in place (natives included), run
    /// the handler of the xpcall that will catch it and return the value
    /// that replaces the error. An error nothing in this thread will catch
    /// keeps its traceback for the host and for `debug.traceback` of a dead
    /// coroutine.
    pub(crate) fn raise_to_handler(&mut self, err: Value) -> Value {
        let raised_by = self.take_errored_natives(err);
        let base = self.running_natives.len();
        for e in raised_by.iter().rev() {
            self.running_natives.push(e.act);
        }
        let catcher = self.nearest_catcher();
        let to_host = catcher.is_none() && self.current.is_none() && self.keep_error_traceback;
        let mut handled = false;
        let out = match catcher {
            Some(Some(handler)) if !self.msgh_applied.is_some_and(|v| v.raw_eq(err)) => {
                handled = true;
                self.call_msgh(handler, err)
            }
            None => {
                if self.keep_error_traceback && self.error_traceback.is_none() {
                    self.error_traceback = Some(self.level_lines());
                }
                err
            }
            Some(_) => err,
        };
        // 5.5 `luaG_errormsg` names a nil error object after any handler
        // ran; an error reaching the host keeps it for lua.c's handler
        let out = if out.is_nil() && self.version() >= LuaVersion::Lua55 && !to_host {
            Value::Str(self.heap.intern(b"<no error object>"))
        } else {
            out
        };
        // the value that leaves here is the handled one: it must not be
        // handled again as it unwinds past further frames
        if handled {
            self.msgh_applied = Some(out);
        }
        self.running_natives.truncate(base);
        out
    }

    /// The message a chunk that failed to compile leaves, `load`'s second
    /// result: the syntax error positioned with `luaO_chunkid`. 5.4 on raise
    /// the parser's "C stack overflow" as a runtime error (`luaE_checkcstack`
    /// through `luaG_errormsg`), inside a protected parser that keeps the
    /// running message handler: the handler of the protected call an error
    /// raised here would reach runs on it, as it would on any error, and its
    /// result is the message (lua.c's handler appends a traceback).
    pub(crate) fn load_error_value(
        &mut self,
        e: &crate::frontend::error::SyntaxError,
        chunkname: &[u8],
    ) -> Value {
        let id = crate::vm::callstack::syntax_chunk_id(self.version(), chunkname);
        let msg = Value::Str(self.heap.intern(&e.render(&id)));
        let stack_overflow = e.line == 0 && e.msg == b"C stack overflow";
        if self.version() < LuaVersion::Lua54 || !stack_overflow {
            return msg;
        }
        match self.nearest_catcher() {
            Some(Some(handler)) => self.call_msgh(handler, msg),
            _ => msg,
        }
    }

    /// Run an xpcall message handler on `err`, as PUC's `luaG_errormsg`
    /// does: with the handler still installed, so an error the handler
    /// raises runs it again at that point (nested, the raising frames still
    /// on the stack), and what that inner run returns is the error thrown
    /// out of the outer one. At `MAX_C_DEPTH` nested runs the error becomes
    /// "C stack overflow", handled once more without re-entry; if the
    /// handler fails on that too, "error in error handling" (errors.lua
    /// :637).
    pub(crate) fn call_msgh(&mut self, handler: Value, err: Value) -> Value {
        // ≤5.2 `luaG_errormsg` raises LUA_ERRERR at once when the handler
        // is not a function
        if self.version() <= LuaVersion::Lua52
            && !matches!(handler, Value::Closure(_) | Value::Native(_))
        {
            return Value::Str(self.heap.intern(b"error in error handling"));
        }
        let capped = self.msgh_depth >= crate::vm::exec::MAX_C_DEPTH;
        let (arg, reenter) = if capped {
            (Value::Str(self.heap.intern(b"C stack overflow")), None)
        } else {
            (err, Some(handler))
        };
        self.msgh_runs += 1;
        let runs = self.msgh_runs;
        self.msgh_depth += 1;
        let r = self.call_protected_with(handler, &[arg], reenter);
        match r {
            Ok(results) => {
                self.msgh_depth -= 1;
                results.first().copied().unwrap_or(Value::Nil)
            }
            Err(_) if capped => {
                self.msgh_depth -= 1;
                Value::Str(self.heap.intern(b"error in error handling"))
            }
            // already the result of the handler run nested at that error
            Err(e) if self.msgh_runs != runs => {
                self.msgh_depth -= 1;
                e.0
            }
            // raised with no Lua frame to unwind (a native handler such as
            // `error` failing at once): no nested run saw it, so run the
            // handler on it here, one level deeper
            Err(e) => {
                let r = self.call_msgh(handler, e.0);
                self.msgh_depth -= 1;
                r
            }
        }
    }

    /// `call_value` as a protected call made from Rust (PUC `lua_pcall`
    /// with no handler): errors inside it do not reach the handler of an
    /// enclosing xpcall, and its error bookkeeping does not outlive it.
    pub(crate) fn call_protected(
        &mut self,
        f: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        self.call_protected_with(f, args, None)
    }

    /// [`Vm::call_protected`] with `handler` as the message handler that is
    /// running while `f` runs (`L->errfunc` during `luaG_errormsg`'s call).
    fn call_protected_with(
        &mut self,
        f: Value,
        args: &[Value],
        handler: Option<Value>,
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        let running = std::mem::replace(&mut self.msgh_running, handler);
        let floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let applied = self.msgh_applied.take();
        let traceback = self.error_traceback.take();
        let natives = std::mem::take(&mut self.errored_natives);
        let keep = std::mem::replace(&mut self.keep_error_traceback, false);
        let r = self.call_value(f, args);
        self.keep_error_traceback = keep;
        self.msgh_running = running;
        self.msgh_floor = floor;
        self.msgh_applied = applied;
        self.error_traceback = traceback;
        self.errored_natives = natives;
        r
    }
}
