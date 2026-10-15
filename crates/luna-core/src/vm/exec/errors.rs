//! Building runtime errors: messages, variable info for the culprit and
//! position prefixes.

use super::*;
use crate::runtime::ErrorStatus;
use crate::runtime::puc_fmt::FmtArg;
mod type_err;
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
        self.rt_errf(format_args!("{msg}"))
    }

    /// [`Vm::rt_err`] with a message to format.
    pub(crate) fn rt_errf(&mut self, msg: std::fmt::Arguments<'_>) -> LuaError {
        self.rt_errf_conv(msg, 0, false)
    }

    /// [`Vm::rt_errf`] for a message PUC formats with `conv` conversions,
    /// after (5.3, `varinfo`) the operand's name was formatted and pushed:
    /// the pushes that format it may find the stack full (see
    /// `format_pushes`).
    pub(crate) fn rt_errf_conv(
        &mut self,
        msg: std::fmt::Arguments<'_>,
        conv: u32,
        varinfo: bool,
    ) -> LuaError {
        if let Some(e) = self.runerror_pushes(conv, varinfo) {
            return e;
        }
        self.varinfo_pushed = false;
        // `luaG_runerror` pushes the message (a native's stack only: a
        // Lua frame's handler goes above its window)
        self.native_push_if_native(1);
        let pos = self.error_position();
        self.raise_text(pos, msg)
    }

    /// [`Vm::raise_msg`] of a message luna formatted itself: PUC's format
    /// is taken to be the text, which a message without a conversion is.
    fn raise_text(&mut self, pos: Option<ErrPos>, msg: std::fmt::Arguments<'_>) -> LuaError {
        let text = msg.to_string();
        if text.contains('%') {
            self.raise_msg(pos, b"%s", &[FmtArg::S(text.as_bytes())])
        } else {
            self.raise_msg(pos, text.as_bytes(), &[])
        }
    }

    /// PUC `luaG_runerror(L, fmt, ...)`: the message made as
    /// `luaO_pushvfstring` makes it, then, in a Lua function, the string
    /// `luaG_addinfo` makes of its position and the message.
    pub(crate) fn raise_msg(
        &mut self,
        pos: Option<ErrPos>,
        fmt: &[u8],
        args: &[FmtArg<'_>],
    ) -> LuaError {
        let l55 = self.version >= LuaVersion::Lua55;
        let h = &mut self.heap;
        let r = h.pushfstring(fmt, args).and_then(|msg| {
            let msg = msg.as_bytes().to_vec();
            match pos {
                None => h.try_intern(&msg),
                Some(ErrPos::At(src, line)) => h.pushfstring(
                    b"%s:%d: %s",
                    &[FmtArg::S(&src), FmtArg::D(line), FmtArg::S(&msg)],
                ),
                Some(ErrPos::NoSource) if l55 => h.pushfstring(b"?:?: %s", &[FmtArg::S(&msg)]),
                Some(ErrPos::NoSource) => h.pushfstring(
                    b"%s:%d: %s",
                    &[FmtArg::S(b"?"), FmtArg::D(-1), FmtArg::S(&msg)],
                ),
            }
        });
        match r {
            Ok(s) => LuaError(Value::Str(s)),
            Err(e) => LuaError::from(e),
        }
    }

    /// Error without the `chunk:line:` position prefix. PUC's
    /// `resume_error` (ldo.c) pushes its message as a bare literal,
    /// so `cannot resume dead coroutine` etc. must not be prefixed.
    pub(crate) fn plain_err(&mut self, msg: &str) -> LuaError {
        self.plain_errf(format_args!("{msg}"))
    }

    /// [`Vm::plain_err`] with a message to format.
    pub(crate) fn plain_errf(&mut self, msg: std::fmt::Arguments<'_>) -> LuaError {
        self.native_push_if_native(1);
        self.raise_text(None, msg)
    }

    /// An error whose object is `prefix` then `msg` as a string; a memory
    /// error when that string cannot be made (PUC raises the memory error
    /// in place of a message it cannot allocate). Nothing is counted as
    /// pushed on the running native's stack.
    pub(crate) fn error_from(
        &mut self,
        prefix: Option<&str>,
        msg: std::fmt::Arguments<'_>,
    ) -> LuaError {
        let s = match prefix {
            Some(p) => self.fmt_str(format_args!("{p}{msg}")),
            None => self.fmt_str(msg),
        };
        match s {
            Ok(s) => LuaError(Value::Str(s)),
            Err(e) => e,
        }
    }

    /// PUC's LUA_ERRERR: the message handler itself failed, and the error
    /// object becomes "error in error handling" (`luaD_seterrorobj`).
    pub(crate) fn errerr(&mut self) -> Value {
        self.errerr_raised += 1;
        let v = match self.alloc_str(b"error in error handling") {
            Ok(s) => Value::Str(s),
            Err(e) => e.0,
        };
        self.handler_bypass = Some(v);
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

    /// Position prefix of the currently executing Lua frame. PUC `luaL_error`
    /// calls `luaL_where(L, 1)` which reads `L->ci->previous`. When the prior
    /// frame is a C function (e.g. a pcall Cont parked above `require`'s
    /// native call), PUC pushes no prefix — match that by looking only at the
    /// topmost frame directly and bailing if it is anything but a Lua frame.
    pub(crate) fn position_prefix(&self) -> Option<String> {
        Some(match self.error_position()? {
            ErrPos::At(src, line) => format!("{}:{line}: ", String::from_utf8_lossy(&src)),
            ErrPos::NoSource => self.stripped_prefix(),
        })
    }

    /// Where `luaG_addinfo` positions an error raised now: `None` when the
    /// running function is not a Lua one.
    pub(crate) fn error_position(&self) -> Option<ErrPos> {
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
        Some(self.pos_at(f.closure.proto, f.pc as usize))
    }

    /// The position of the instruction before `pc` in `proto`.
    pub(super) fn pos_at(&self, proto: Gc<crate::runtime::function::Proto>, pc: usize) -> ErrPos {
        // a stripped chunk: no source in luna's own format, no line info in
        // PUC's (whose loader names the missing source "=?")
        if proto.source.as_bytes().is_empty() || proto.lines.is_empty() {
            return ErrPos::NoSource;
        }
        let line = proto.lines[pc.saturating_sub(1).min(proto.lines.len() - 1)];
        let raw = proto.source.as_bytes();
        ErrPos::At(crate::vm::lib_debug::chunk_id(self.version, raw), i64::from(line))
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

    /// The `short_src` and current line `luaL_where(L, level)` reports.
    pub(crate) fn position_at_level(&self, level: i64) -> Option<(Vec<u8>, i64)> {
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
        Some((ar.short_src, line))
    }
}

/// Where an error is raised: a chunk's id and a line, or a function with
/// no source.
pub(crate) enum ErrPos {
    At(Vec<u8>, i64),
    NoSource,
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
