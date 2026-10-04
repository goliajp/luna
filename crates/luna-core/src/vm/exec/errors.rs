//! Building runtime errors: messages, variable info for the culprit and
//! position prefixes.

use super::*;
use crate::runtime::ErrorStatus;

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
        LuaError(Value::Str(self.heap.intern(msg.as_bytes())))
    }

    /// PUC's LUA_ERRERR: the message handler itself failed, and the error
    /// object becomes "error in error handling" (`luaD_seterrorobj`).
    pub(crate) fn errerr(&mut self) -> Value {
        self.errerr_raised += 1;
        Value::Str(self.heap.intern(b"error in error handling"))
    }

    /// PUC's LUA_ERRMEM: an allocation was refused, and the error object is
    /// "not enough memory".
    pub(crate) fn mem_err(&mut self) -> LuaError {
        self.mem_raised += 1;
        self.plain_err("not enough memory")
    }

    /// How many errors have taken a status of their own so far; see
    /// [`Vm::error_status`].
    #[doc(hidden)]
    pub fn special_errors(&self) -> SpecialErrors {
        SpecialErrors {
            errerr: self.errerr_raised,
            gcmm: self.gcmm_raised,
            mem: self.mem_raised,
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
        self.runerror(&msg)
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

    /// Name the offending operand of the current instruction (PUC varinfo) for
    /// a type error, e.g. " (global 'x')". The faulting value `bad` is matched
    /// to the instruction's subject register(s); a native-raised error whose
    /// current instruction doesn't hold `bad` simply yields "".
    pub(super) fn subject_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
        // PUC `varinfo` names a variable only for a Lua activation
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.last().and_then(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        let mut cands: Vec<u32> = Vec::new();
        match instr.op() {
            // indexed reads / length / method: the table/object is in B
            Op::GetField | Op::GetI | Op::GetTable | Op::SelfOp | Op::Len => {
                cands.push(instr.b());
            }
            // indexed writes / calls: the table/function is in A
            Op::SetField | Op::SetI | Op::SetTable | Op::Call | Op::TailCall => {
                cands.push(instr.a());
            }
            // arithmetic/bitwise: a register operand (B, and C unless constant)
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Pow
            | Op::IDiv
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr => {
                cands.push(instr.b());
                if !instr.k() {
                    cands.push(instr.c());
                }
            }
            Op::Unm | Op::BNot => cands.push(instr.b()),
            // arithmetic on a constant or an immediate: the register operand
            Op::AddI
            | Op::SubI
            | Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::ModK
            | Op::PowK
            | Op::DivK
            | Op::IDivK
            | Op::BAndK
            | Op::BOrK
            | Op::BXorK
            | Op::ShrI
            | Op::ShlI => cands.push(instr.b()),
            // indexing an upvalue table (`_ENV` for a global): PUC
            // `getupvalname` finds the value among the closure's upvalues
            Op::GetTabUp | Op::SetTabUp => {
                let u = if instr.op() == Op::GetTabUp {
                    instr.b()
                } else {
                    instr.a()
                };
                if self.upval_get(f.closure, u).raw_eq(bad)
                    && let Some(d) = p.upvals.get(u as usize)
                {
                    return format!(" (upvalue '{}')", d.name);
                }
            }
            Op::Concat => {
                let a = instr.a();
                for r in a..a + instr.b() {
                    cands.push(r);
                }
            }
            _ => {}
        }
        // Up to 5.3 a binary operator takes a constant operand straight
        // from the constant table (RK), where `varinfo` cannot see it, so a
        // string constant is not named there; unary operators load it into
        // a register first and do name it.
        let rk_operands = self.version <= LuaVersion::Lua53
            && matches!(
                instr.source_op(),
                Op::Add
                    | Op::Sub
                    | Op::Mul
                    | Op::Div
                    | Op::Mod
                    | Op::Pow
                    | Op::IDiv
                    | Op::BAnd
                    | Op::BOr
                    | Op::BXor
                    | Op::Shl
                    | Op::Shr
            );
        for reg in cands {
            if self.r(f.base, reg).raw_eq(bad) {
                return match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                    Some(("constant", _)) if rk_operands => String::new(),
                    Some((kind, name)) => format!(" ({kind} '{name}')"),
                    None => String::new(),
                };
            }
        }
        String::new()
    }

    /// "attempt to call a X value", enriched (PUC luaG_callerror) with a name
    /// for the call target: "(global 'f')" for a direct call, or "(metamethod
    /// 'add')" when the call is a metamethod dispatched by the current opcode.
    pub(super) fn call_err(&mut self, v: Value) -> LuaError {
        let extra = self.call_target_varinfo(v);
        let tn = self.obj_typename(v);
        let msg = self.compose_type_err("call", &tn, &extra);
        self.runerror(&msg)
    }

    /// Name the offending call target. A metamethod dispatch pushes a `Cont`
    /// frame before the call, so the opcode that triggered it lives in the
    /// nearest *Lua* frame — read that instruction: OP_CALL names the function
    /// register, any metamethod-bearing opcode yields "(metamethod 'event')".
    pub(super) fn call_target_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
        // a hook's call (PUC `funcnamefromcall` on a `CIST_HOOKED` caller)
        if self.pending_is_hook && self.version >= LuaVersion::Lua54 {
            return " (hook '?')".to_string();
        }
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.iter().rev().find_map(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        match instr.source_op() {
            Op::Call | Op::TailCall => {
                let reg = instr.a();
                if self.r(f.base, reg).raw_eq(bad) {
                    match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                        Some((kind, name)) => format!(" ({kind} '{name}')"),
                        None => String::new(),
                    }
                } else {
                    String::new()
                }
            }
            // 5.4 `funcnamefromcode` names the generic-for iterator call
            // (5.3 had the entry but raised through plain `luaG_typeerror`)
            Op::TForCall if self.version >= LuaVersion::Lua54 => {
                " (for iterator 'for iterator')".to_string()
            }
            // 5.4 `funcnamefromcall` names the metamethod; up to 5.3 the
            // call raised through `luaG_typeerror`, whose `varinfo` does not
            op if self.version >= LuaVersion::Lua54 => match mm_event_name(op) {
                Some(ev) => format!(" (metamethod '{ev}')"),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// "number has no integer representation", enriched (PUC luaG_tointerror)
    /// with a "(field 'x')"-style suffix naming the offending operand of the
    /// current arithmetic instruction when it can be recovered from bytecode.
    pub(super) fn no_int_rep_err(&mut self) -> LuaError {
        let extra = self.bad_operand_varinfo();
        self.runerror(&format!("number{extra} has no integer representation"))
    }

    /// Inspect the current frame's faulting instruction: find the register
    /// operand holding a float with no integer representation and name it.
    pub(super) fn bad_operand_varinfo(&self) -> String {
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.last().and_then(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        let mut regs = vec![instr.b()];
        // C of a constant- or immediate-operand opcode is not a register
        if !instr.k() && instr.arith_const_op().is_none() {
            regs.push(instr.c());
        }
        let no_int = |n: Option<Num>| matches!(n, Some(Num::Float(x)) if crate::runtime::value::f2i_exact(x).is_none());
        for reg in regs {
            let v = self.r(f.base, reg);
            // before 5.4 a numeric string is converted first, so "2.5" is
            // the operand without an integer value
            let n = self.arith_operand()(v);
            if no_int(n) {
                return match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                    Some((kind, name)) => format!(" ({kind} '{name}')"),
                    None => String::new(),
                };
            }
        }
        String::new()
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
        let proto = f.closure.proto;
        // a stripped chunk: no source in luna's own format, no line info in
        // PUC's (whose loader names the missing source "=?")
        if proto.source.as_bytes().is_empty() || proto.lines.is_empty() {
            return Some(self.stripped_prefix());
        }
        let line = proto.lines[(f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1)];
        let raw = proto.source.as_bytes();
        let display = crate::vm::lib_debug::chunk_id(self.version, raw);
        let src = String::from_utf8_lossy(&display).into_owned();
        Some(format!("{src}:{line}: "))
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
