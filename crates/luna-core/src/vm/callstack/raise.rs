//! Raising an error: its position, the natives it passed through, and
//! the handler that catches it.

use crate::runtime::Value;
use crate::runtime::function::{CallFrame, ContKind};
use crate::runtime::mem::LVec;
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
                .is_some_and(|a| a.depth() as usize == self.frames.len())
    }

    /// Remember that `nc` raised `err`. A native only leaves the stack by
    /// the time the error reaches `unwind`, where PUC still has it; natives
    /// an error passes through on its way out collect innermost first, and
    /// a different error starts the list over.
    pub(crate) fn note_errored_native(&mut self, act: NativeAct, err: Value) {
        let continues = self
            .errored_natives
            .last()
            .is_some_and(|inner| inner.err.raw_eq(err) && inner.act.depth() >= act.depth());
        if !continues {
            self.errored_natives.clear();
        }
        self.errored_natives
            .push_or_abort(ErroredNative { act, err });
    }

    /// An error a native raised when the host called it directly (no Lua
    /// function between them) reaches the host without unwinding a Lua
    /// frame, the point where `raise_to_handler` usually runs: run it here,
    /// with that native still counted, so the host gets its traceback.
    pub(crate) fn raise_native_to_host(&mut self, err: Value) {
        let raised_here = self
            .errored_natives
            .last()
            .is_some_and(|e| e.err.raw_eq(err) && e.act.depth() as usize == self.frames.len());
        if raised_here && self.error_traceback.is_none() {
            self.raise_to_handler(err);
        }
    }

    /// The natives recorded for `err` that were running at the top of the
    /// stack, innermost first.
    fn take_errored_natives(&mut self, err: Value) -> LVec<ErroredNative> {
        let mut list = self.errored_natives.take();
        let depth = self.frames.len() as u32;
        if !list
            .iter()
            .all(|e| e.err.raw_eq(err) && e.act.depth() == depth)
        {
            list.clear();
        }
        list
    }

    /// "attempt to call", raised (PUC `luaG_callerror`) with the call's
    /// function and arguments ending at `top`, where the name of the
    /// value and the message go, and before 5.4 the positioned message
    /// a Lua caller adds.
    pub(crate) fn call_err_at(&mut self, v: Value, top: u32) -> crate::vm::error::LuaError {
        let lua = !self.native_on_top();
        let e = self.call_err(v);
        let positioned = u32::from(lua && self.version() < LuaVersion::Lua54);
        self.overflow_top = Some(top + u32::from(self.varinfo_pushed) + positioned);
        e
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
                    ContKind::Pcall { .. } => Some(None),
                    ContKind::Xpcall { handler, .. } => Some(Some(handler)),
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
        // LUA_ERRERR is thrown past the handler
        if self.errerr_in_flight.take().is_some_and(|v| v.raw_eq(err)) {
            return err;
        }
        let raised_by = self.take_errored_natives(err);
        let at = self.raise_top(&raised_by);
        let base = self.running_natives.len();
        for e in raised_by.iter().rev() {
            self.running_natives.push_or_abort(e.act);
        }
        let catcher = self.nearest_catcher();
        let to_host = catcher.is_none() && self.current.is_none() && self.keep_error_traceback;
        let mut handled = false;
        let out = match catcher {
            Some(Some(handler)) if !self.msgh_applied.is_some_and(|v| v.raw_eq(err)) => {
                handled = true;
                self.call_msgh_at(handler, err, at)
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

    /// The slot `luaG_errormsg` runs the message handler at: where the
    /// error object was, on top of the stack that raised it. A native
    /// raises with that object on top of its own stack, which `top`
    /// follows as PUC's does (see `Vm::native_push`). A Lua frame raises
    /// at its `L->top`, the top of the call the stack overflowed on or else
    /// its whole window: `luaG_runerror` pushes the message there, and
    /// before 5.4 the positioned message as well, which it does not pop;
    /// 5.3+'s `varinfo` has pushed the operand's name before them. Where
    /// the error came from no frame, the handler runs where the stack ends.
    fn raise_top(&mut self, raised_by: &[ErroredNative]) -> Option<u32> {
        if let Some(at) = self.overflow_top.take() {
            return Some(at);
        }
        let positioned_message = u32::from(self.version() < LuaVersion::Lua54);
        if let Some(e) = raised_by.first() {
            return Some(e.act.top() - 1);
        }
        let varinfo = u32::from(self.varinfo_pushed);
        self.frames
            .iter()
            .rev()
            .find_map(CallFrame::lua)
            .map(|f| f.base + f.closure.proto.max_stack as u32 + positioned_message + varinfo)
    }

    /// Run an xpcall message handler on `err`, as PUC's `luaG_errormsg`
    /// does: with the handler still installed, so an error the handler
    /// raises runs it again at that point (nested, the raising frames still
    /// on the stack), and what that inner run returns is the error thrown
    /// out of the outer one. The handler's calls take C levels like any
    /// other: past `MAX_C_DEPTH` the handler is run on the "C stack
    /// overflow" that refusing one raises, and at `errerr_c_depth` the
    /// refusal is "error in error handling", which no handler runs on
    /// (errors.lua :637).
    pub(crate) fn call_msgh(&mut self, handler: Value, err: Value) -> Value {
        self.call_msgh_at(handler, err, None)
    }

    /// [`Vm::call_msgh`] with the handler called at stack slot `at` (PUC
    /// `luaG_errormsg`, see `raise_top`).
    pub(crate) fn call_msgh_at(&mut self, handler: Value, err: Value, at: Option<u32>) -> Value {
        // 5.4+ `lua_error` raises the memory error message itself as a
        // memory error, which no handler runs on
        if self.version() >= LuaVersion::Lua54
            && let Value::Str(s) = err
            && s.as_bytes() == b"not enough memory"
        {
            self.heap.mem_ctx().raise_oom();
            return err;
        }
        // ≤5.2 `luaG_errormsg` raises LUA_ERRERR at once when the handler
        // is not a function
        if self.version() <= LuaVersion::Lua52
            && !matches!(handler, Value::Closure(_) | Value::Native(_))
        {
            return self.errerr();
        }
        self.msgh_runs += 1;
        let runs = self.msgh_runs;
        let errerrs = self.errerr_raised;
        // a call refused at the C-level limit keeps its level while its
        // handler runs (PUC's `luaD_call` never takes it back)
        let held = self.c_overflow_err.take().is_some_and(|v| v.raw_eq(err));
        self.g.nccalls += u32::from(held);
        self.msgh_depth += 1;
        let r = self.call_protected_with(handler, &[err], Some(handler), at);
        self.msgh_depth -= 1;
        self.g.nccalls -= u32::from(held);
        match r {
            Ok(results) => results.first().copied().unwrap_or(Value::Nil),
            // the handler's own call was refused with LUA_ERRERR
            Err(e) if self.msgh_runs == runs && self.errerr_raised != errerrs => e.0,
            // already the result of the handler run nested at that error
            Err(e) if self.msgh_runs != runs => e.0,
            // raised with no Lua frame to unwind (a native handler such as
            // `error` failing at once, or the call refused at the C-level
            // limit): no nested run saw it, so run the handler on it here,
            // one level deeper
            Err(e) => self.call_msgh(handler, e.0),
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
        self.call_protected_with(f, args, None, None)
    }

    /// [`Vm::call_protected`] with `handler` as the message handler that is
    /// running while `f` runs (`L->errfunc` during `luaG_errormsg`'s call).
    fn call_protected_with(
        &mut self,
        f: Value,
        args: &[Value],
        handler: Option<Value>,
        at: Option<u32>,
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        let running = std::mem::replace(&mut self.msgh_running, handler);
        let floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let applied = self.msgh_applied.take();
        let traceback = self.error_traceback.take();
        let natives = self.errored_natives.take();
        let keep = std::mem::replace(&mut self.keep_error_traceback, false);
        let r = self.call_value_impl(f, args, false, at);
        self.keep_error_traceback = keep;
        self.msgh_running = running;
        self.msgh_floor = floor;
        self.msgh_applied = applied;
        self.error_traceback = traceback;
        self.errored_natives = natives;
        r
    }
}
