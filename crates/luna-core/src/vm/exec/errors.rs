//! Building runtime errors: messages, variable info for the culprit and
//! position prefixes.

use super::*;
use crate::runtime::ErrorStatus;
mod varinfo;

impl Vm {
    /// Take the traceback of the latest error that reached the host, and
    /// reset it. Embedders should call this immediately after a failed
    /// `call_value`/`eval`/`call`/etc. — the next public `call_value`
    /// entry clears it. Returns `None` if no error was in flight.
    ///
    /// The text is what PUC's `luaL_traceback(L, L, NULL, 1)` returns in a
    /// message handler of the host's `lua_pcall` (5.1: `debug.traceback`
    /// called there with level 2), taken where the error was raised:
    /// `stack traceback:` and then one `\n\t` line per stack level,
    /// innermost first, C functions included. The first level is the
    /// function that raised: `\n\t[C]: in function 'error'` for `error`,
    /// the library function for an argument error, or the Lua function for
    /// an error raised by an operation. A Lua level reads
    /// `\n\t<short_src>:<line>: in <name>`, a C level `\n\t[C]: in <name>`.
    /// Levels are left out as `luaL_traceback` leaves them out of a deep
    /// stack, and names follow the dialect, as the embedding guide lists.
    pub fn take_error_traceback(&mut self) -> Option<String> {
        let levels = self.error_traceback.take()?;
        let mut tb = b"stack traceback:".to_vec();
        // the handler's own level (5.1: and `debug.traceback`'s) sits above
        // the stack that raised
        let hidden = if self.version == LuaVersion::Lua51 {
            2
        } else {
            1
        };
        tb.extend(crate::vm::callstack::traceback_from_lines(
            self.version,
            &levels,
            0,
            hidden,
        ));
        Some(String::from_utf8_lossy(&tb).into_owned())
    }

    /// PUC `luaL_traceback(L, L, msg, level)` on the running thread: `msg`
    /// (when given) and a newline, then `stack traceback:` and one line per
    /// stack level from `level` on, level 0 being the running function (the
    /// native calling this, when a native does).
    pub fn traceback(&mut self, msg: Option<&[u8]>, level: i64) -> Vec<u8> {
        let mut out = match msg {
            Some(m) => {
                let mut out = m.to_vec();
                out.push(b'\n');
                out
            }
            None => Vec::new(),
        };
        out.extend_from_slice(b"stack traceback:");
        out.extend(self.traceback_lines(None, level));
        out
    }

    #[doc(hidden)]
    pub fn rt_err(&mut self, msg: &str) -> LuaError {
        self.varinfo_pushed = false;
        // `luaG_runerror` pushes the message (a native's stack only: a
        // Lua frame's handler goes above its window)
        self.native_push_if_native(1);
        let text = match self.position_prefix() {
            Some(p) => format!("{p}{msg}"),
            None => msg.to_string(),
        };
        LuaError(Value::Str(self.heap.intern(text.as_bytes())))
    }

    /// Error without the `chunk:line:` position prefix. PUC's
    /// `resume_error` (ldo.c) pushes its message as a bare literal,
    /// so `cannot resume dead coroutine` etc. must not be prefixed.
    pub(crate) fn plain_err(&mut self, msg: &str) -> LuaError {
        self.native_push_if_native(1);
        LuaError(Value::Str(self.heap.intern(msg.as_bytes())))
    }

    /// PUC's LUA_ERRERR: the message handler itself failed, and the error
    /// object becomes "error in error handling" (`luaD_seterrorobj`).
    pub(crate) fn errerr(&mut self) -> Value {
        self.errerr_raised += 1;
        let v = Value::Str(self.heap.intern(b"error in error handling"));
        self.errerr_in_flight = Some(v);
        v
    }

    /// PUC's LUA_ERRMEM: an allocation was refused, and the error object is
    /// "not enough memory".
    pub(crate) fn mem_err(&mut self) -> LuaError {
        LuaError(Value::Str(self.heap.mem_ctx().raise_oom()))
    }

    /// How many errors have taken a status of their own so far; see
    /// [`Vm::error_status`].
    #[doc(hidden)]
    pub fn special_errors(&self) -> SpecialErrors {
        SpecialErrors {
            errerr: self.errerr_raised,
            gcmm: self.gcmm_raised,
            mem: self.heap.mem_ctx().oom_raised(),
        }
    }

    /// The status of error `e`, raised after `before` was taken: an error
    /// the vm raised itself with a status of its own carries that status's
    /// message, so a message the program raised with the same text, and no
    /// such error raised meanwhile, stays `Run`.
    #[doc(hidden)]
    pub fn error_status(&self, e: Value, before: SpecialErrors) -> ErrorStatus {
        let now = self.special_errors();
        let Value::Str(s) = e else {
            return ErrorStatus::Run;
        };
        let s = s.as_bytes();
        let v52 = matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53);
        if now.errerr != before.errerr && s == b"error in error handling" {
            ErrorStatus::Err
        } else if now.mem != before.mem && s == b"not enough memory" {
            ErrorStatus::Mem
        } else if v52 && now.gcmm != before.gcmm && s.starts_with(b"error in __gc metamethod (") {
            ErrorStatus::Gcmm
        } else {
            ErrorStatus::Run
        }
    }

    /// A string a library built from pieces of any size: one longer than a
    /// string can hold raises, as the concatenation operator does.
    pub(crate) fn built_str(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        if bytes.len() > crate::runtime::string::MAX_LEN {
            return Err(self.rt_err("string length overflow"));
        }
        Ok(Value::Str(self.heap.intern(bytes)))
    }

    pub(crate) fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        let extra = self.subject_varinfo(v);
        let tn = self.obj_typename(v);
        let msg = self.compose_type_err(what, &tn, &extra);
        self.runerror_named(&msg, &extra)
    }

    /// [`Self::runerror`] for a message that names an operand with `extra`
    /// (see `varinfo_pushed`).
    fn runerror_named(&mut self, msg: &str, extra: &str) -> LuaError {
        let e = self.runerror(msg);
        self.varinfo_pushed = self.version() >= LuaVersion::Lua53 && !extra.is_empty();
        e
    }

    /// Assemble a `luaG_typeerror` / `luaG_callerror` message in the dialect's
    /// word order.
    ///
    /// PUC ≤5.2 names the operand first — `attempt to call field 'f' (a nil
    /// value)`. 5.3 flipped it to type-first — `attempt to call a nil value
    /// (field 'f')`. luna emitted the 5.3+ form on every dialect, so every
    /// such error was worded wrong under 5.1/5.2.
    ///
    /// Two shapes carry no operand name on ≤5.2 and must collapse to the bare
    /// message: an absent varinfo (identical across dialects), and a
    /// metamethod target — ≤5.2's `luaG_typeerror` only names locals, globals,
    /// fields, upvalues and methods, so `(metamethod 'add')` has no ≤5.2
    /// counterpart and is dropped rather than reworded. All four shapes were
    /// measured against stock 5.1.5 / 5.2.4 / 5.5.1 before this was written.
    pub(super) fn compose_type_err(&self, what: &str, tn: &str, extra: &str) -> String {
        if self.version() > crate::version::LuaVersion::Lua52 {
            return format!("attempt to {what} a {tn} value{extra}");
        }
        // `extra` is "" or " (kind 'name')" — unwrap to "kind 'name'".
        let inner = extra
            .trim_start()
            .trim_start_matches('(')
            .trim_end_matches(')');
        if inner.is_empty() || inner.starts_with("metamethod") {
            format!("attempt to {what} a {tn} value")
        } else {
            format!("attempt to {what} {inner} (a {tn} value)")
        }
    }

    /// Position prefix of the currently executing Lua frame. PUC `luaL_error`
    /// calls `luaL_where(L, 1)` which reads `L->ci->previous`. When the prior
    /// frame is a C function (e.g. a pcall Cont parked above `require`'s
    /// native call), PUC pushes no prefix — match that by looking only at the
    /// topmost frame directly and bailing if it is anything but a Lua frame.
    pub(crate) fn position_prefix(&self) -> Option<String> {
        let f = match self.frames.last()? {
            CallFrame::Lua(f) => f,
            // a native metamethod runs above the Meta continuation of the
            // instruction that triggered it: that Lua function is its caller
            CallFrame::Cont(NativeCont {
                kind: ContKind::Meta(_),
                ..
            }) => self.frames.iter().rev().nth(1)?.lua()?,
            CallFrame::Cont(_) => return None,
        };
        Some(self.prefix_at(f.closure.proto, f.pc as usize))
    }

    /// `"short_src:line: "` of the instruction before `pc` in `proto`.
    pub(super) fn prefix_at(
        &self,
        proto: Gc<crate::runtime::function::Proto>,
        pc: usize,
    ) -> String {
        // a stripped chunk: no source in luna's own format, no line info in
        // PUC's (whose loader names the missing source "=?")
        if proto.source.as_bytes().is_empty() || proto.lines.is_empty() {
            return self.stripped_prefix();
        }
        let line = proto.lines[pc.saturating_sub(1).min(proto.lines.len() - 1)];
        let raw = proto.source.as_bytes();
        let display = crate::vm::lib_debug::chunk_id(self.version, raw);
        let src = String::from_utf8_lossy(&display).into_owned();
        format!("{src}:{line}: ")
    }

    /// PUC `luaG_addinfo` prefix for a stripped chunk. 5.5 substitutes "=?"
    /// for the source and renders the line as "?" (so the prefix reads
    /// `?:?: `). 5.4 and below leave the source NULL ("?") and use the raw
    /// `getfuncline = -1`, so the prefix reads `?:-1: ` (5.4 errors.lua :282
    /// matches `^%?:%-1:`).
    pub(super) fn stripped_prefix(&self) -> String {
        if self.version >= crate::version::LuaVersion::Lua55 {
            "?:?: ".to_string()
        } else {
            "?:-1: ".to_string()
        }
    }

    /// PUC `luaL_where(L, level)`: `"short_src:line: "` for the function at
    /// `level` (0 = the running native), or `None` when that level does not
    /// exist or has no line information (a C function, a stripped chunk).
    pub(crate) fn position_prefix_at_level(&self, level: i64) -> Option<String> {
        let ts = self.thread_stack(None);
        let i = usize::try_from(level).ok()?;
        if i >= ts.levels.len() {
            return None;
        }
        let line = ts.currentline(i);
        if line <= 0 {
            return None;
        }
        let DbgKind::Lua(fi) = ts.levels[i] else {
            return None;
        };
        let ar = self.closure_ar(ts.lua(fi).closure);
        Some(format!(
            "{}:{line}: ",
            String::from_utf8_lossy(&ar.short_src)
        ))
    }
}

/// Counts of the errors the vm raised with a status of their own (see
/// [`Vm::special_errors`]).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpecialErrors {
    errerr: u64,
    gcmm: u64,
    mem: u64,
}
