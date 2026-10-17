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
        // a record that cannot be kept only leaves the native out of the
        // error's traceback
        let _ = self.errored_natives.push(ErroredNative { act, err });
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
        // the message is formatted from there too
        self.raise_from = Some(top);
        self.overflow_top = None;
        let e = self.call_err(v);
        self.raise_from = None;
        if self.overflow_top.is_some() {
            // formatting it ran out of stack: that overflow is raised
            return e;
        }
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
        // LUA_ERRERR and LUA_ERRGCMM are thrown past the handler, the one
        // the catching xpcall would otherwise call too
        if self.handler_bypass.take().is_some_and(|v| v.raw_eq(err)) {
            self.msgh_applied = Some(err);
            return err;
        }
        let raised_by = self.take_errored_natives(err);
        let at = self.raise_top(&raised_by);
        let base = self.running_natives.len();
        for e in raised_by.iter().rev() {
            // these were popped from `running_natives` just before, whose
            // block kept their room
            self.running_natives.push_or_abort(e.act);
        }
        let catcher = self.nearest_catcher();
        let to_host = catcher.is_none() && self.current.is_none() && self.keep_error_traceback;
        let mut handled = false;
        let out = match catcher {
            Some(Some(handler)) if !self.msgh_applied.is_some_and(|v| v.raw_eq(err)) => {
                handled = true;
                match self.errormsg_room(at) {
                    Ok(()) => self.call_msgh_at(handler, err, at),
                    // the room for the handler ran out: it runs on that
                    Err(e) => self.raise_to_handler(e.0),
                }
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
            self.str_value(b"<no error object>").unwrap_or_else(|e| e.0)
        } else {
            out
        };
        // the value that leaves here is the handled one: it must not be
        // handled again as it unwinds past further frames
        if handled {
            self.msgh_applied = Some(out);
        }
        self.running_natives.truncate(base);
        // the boxes of the natives the error leaves are garbage
        self.buffer_boxes.retain(|&(at, _)| at as usize <= base);
        out
    }

    /// The message a chunk that failed to compile leaves, `load`'s second
    /// result: the syntax error positioned with `luaO_chunkid` (the string
    /// the load's count made, when it made it). 5.4 on raise the parser's
    /// "C stack overflow" as a runtime error inside a protected parser that
    /// keeps the running message handler, whose result is the message.
    pub(crate) fn load_error_value(
        &mut self,
        e: &crate::frontend::error::SyntaxError,
        chunkname: &[u8],
    ) -> Value {
        let text = e.render(&crate::vm::callstack::syntax_chunk_id(
            self.version(),
            chunkname,
        ));
        let msg = match self.heap.puc.load_msg.take() {
            Some(m) if m.as_bytes() == text => Value::Str(m),
            _ => self.str_value(&text).unwrap_or_else(|e| e.0),
        };
        if self.version() < LuaVersion::Lua54 || e.line != 0 || e.msg != b"C stack overflow" {
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
}

impl Vm {
    /// Before a collection's atomic phase: the error values the unwinder
    /// compares by identity, watched without being kept alive.
    pub(crate) fn watch_errors(&mut self) {
        let w = &mut self.heap.weak_watch;
        w.clear();
        for v in [self.msgh_applied, self.handler_bypass] {
            w.push((v.unwrap_or(Value::Nil), false));
        }
        w.extend(self.errored_natives.iter().map(|e| (e.err, false)));
    }

    /// After it: forget each one the collection found dead, before its
    /// object is freed and its address reused by another value.
    pub(crate) fn forget_dead_errors(&mut self) {
        let w = std::mem::take(&mut self.heap.weak_watch);
        if w.first().is_some_and(|x| x.1) {
            self.msgh_applied = None;
        }
        if w.get(1).is_some_and(|x| x.1) {
            self.handler_bypass = None;
        }
        let mut dead = w.iter().skip(2).map(|x| x.1);
        self.errored_natives
            .retain(|_| !dead.next().unwrap_or(false));
    }
}

#[cfg(test)]
#[path = "raise_tests.rs"]
mod tests;
