//! `assert`, `error` and the `luaL_error` / `luaL_argerror` helpers the
//! library modules share.

use super::*;
use crate::runtime::puc_fmt::FmtArg;
use crate::runtime::{Gc, LuaStr};

pub(super) fn nat_assert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = a.get(vm, 0);
    if v.truthy() {
        // assert returns all its arguments: one slot down, over itself
        let (fs, n) = (fs as usize, nargs as usize);
        vm.stack.copy_within(fs + 1..fs + 1 + n, fs);
        return Ok(nargs);
    }
    match vm.version() {
        // 5.1 checks for a condition first; 5.2 does not, so a bare
        // `assert()` fails the assertion. Both format the message with
        // `luaL_optstring` (numbers convert, anything else is an argument
        // error) and raise it through `luaL_error`.
        LuaVersion::Lua51 | LuaVersion::Lua52 => {
            if vm.version() == LuaVersion::Lua51 {
                argcheck::check_any(vm, a, 0)?;
            }
            match argcheck::opt_string(vm, a, 1)? {
                // `luaL_error(L, "%s", msg)`
                Some(msg) => Err(raise_lib(vm, b"%s", &[FmtArg::S(msg.as_bytes())], 1)),
                None => Err(raise_lib(vm, b"%s", &[FmtArg::S(b"assertion failed!")], 1)),
            }
        }
        // 5.3+ hands the message, of any type, to `error` at level 1; an
        // explicit nil message stays nil (`lua_settop(L, 1)` keeps it).
        // The message replaces the arguments (`lua_settop(L, 1)`); `error`
        // then adds the position to a string only.
        _ => {
            argcheck::check_any(vm, a, 0)?;
            vm.native_settop(1);
            match a.get(vm, 1) {
                _ if nargs < 2 => Err(raise_value(vm, "assertion failed!")),
                Value::Str(s) => Err(raise(vm, Message::Pushed(s.as_bytes()), None)),
                v => Err(LuaError(v)),
            }
        }
    }
}

pub(crate) fn nat_error(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // The level is read before anything else, so a bad level is reported
    // whatever the message is.
    let level = argcheck::opt_int(vm, a, 1, 1)?;
    let msg = a.get(vm, 0);
    // A nil error object stays nil HERE: PUC 5.5's luaG_errormsg
    // substitutes "<no error object>" only AFTER the message handler
    // ran (ldebug.c:849-852) — xpcall handlers and the standalone
    // msghandler see the raw nil ("(error object is a nil value)" at
    // top level, v2.14 fixture 5.5/334), while a plain pcall catch
    // yields the substituted string. luna's substitution lives at the
    // matching point in `unwind` (5.5-gated there).
    // `lua_settop(L, 1)`: the message is the one value left
    vm.native_settop(1);
    if level <= 0 {
        return Err(LuaError(msg));
    }
    // ≤5.2 tests `lua_isstring`, so a number message is positioned too and
    // comes out as a string; 5.3+ positions only real strings.
    match msg {
        Value::Str(_) => {}
        Value::Int(_) | Value::Float(_) if vm.version() <= LuaVersion::Lua52 => {}
        v => return Err(LuaError(v)),
    }
    // PUC `luaB_error` calls `luaL_where(L, level)` — prepend the position of
    // the Lua frame `level` steps up. If the level is out of range or the
    // target frame has no line info, fall through with no prefix.
    let pos = vm.position_at_level(level as i64);
    if let Some(e) = vm.format_pushes(&[pos.is_some().then_some(2), None]) {
        return Err(e);
    }
    // `luaL_where` and the message, concatenated into one value
    vm.native_push(1);
    let at = lual_where(vm, pos)?;
    Err(LuaError(concat2(vm, Value::Str(at), msg)?))
}

/// Raise a string-ish error with the caller's position prefix (PUC level 1).
/// PUC `luaL_error`: a string message gets `luaL_where(L, 1)`, the position
/// of whatever called the running native — including a Lua frame that
/// reached it as a metamethod — and nothing when that caller is C. `conv`:
/// the conversions (`%s`, `%d`, ...) of the format PUC formats the message
/// with, whose pieces it pushes one by one (see [`Vm::format_pushes`]);
/// `None`: the message is pushed as it is (`error` and 5.3+ `assert`).
fn raise(vm: &mut Vm, msg: Message<'_>, conv: Option<u32>) -> LuaError {
    let pos = vm.position_at_level(1);
    // `luaL_where`, a formatted position or a literal "", then the message
    let at = pos.is_some().then_some(2);
    if let Some(e) = vm.format_pushes(&[at, conv]) {
        return e;
    }
    // concatenated into one value
    vm.native_push(1);
    let r = (|| {
        // 5.3+ `assert` pushes its message before `luaB_error` positions it
        let early = match msg {
            Message::Pushed(m) => Some(vm.heap.try_intern(m)?),
            Message::Fmt(..) => None,
        };
        let at = lual_where(vm, pos)?;
        let m = match (early, msg) {
            (Some(m), _) => m,
            (None, Message::Fmt(fmt, args)) => vm.heap.pushfstring(fmt, args)?,
            (None, Message::Pushed(_)) => unreachable!("pushed above"),
        };
        concat2(vm, Value::Str(at), Value::Str(m))
    })();
    match r {
        Ok(v) => LuaError(v),
        Err(e) => e,
    }
}

/// A `luaL_error` message: PUC's format and its arguments, or (5.3+
/// `assert`) a string already pushed.
#[derive(Clone, Copy)]
pub(crate) enum Message<'a> {
    Fmt(&'a [u8], &'a [FmtArg<'a>]),
    Pushed(&'a [u8]),
}

/// The format PUC would have given a message luna formatted itself: the
/// text, when it has no `%` in it.
fn literal(msg: &[u8]) -> (&[u8], [FmtArg<'_>; 1], usize) {
    if msg.contains(&b'%') {
        (b"%s", [FmtArg::S(msg)], 1)
    } else {
        (msg, [FmtArg::S(b"")], 0)
    }
}

/// PUC `luaL_where`: the string of position `pos` (`"%s:%d: "`), `""`
/// without one.
fn lual_where(vm: &mut Vm, pos: Option<(Vec<u8>, i64)>) -> Result<Gc<LuaStr>, LuaError> {
    let s = match pos {
        Some((src, line)) => vm
            .heap
            .pushfstring(b"%s:%d: ", &[FmtArg::S(&src), FmtArg::D(line)]),
        None => vm.heap.try_intern(b""),
    };
    s.map_err(LuaError::from)
}

/// `lua_concat(L, 2)` of `a` and `b` (strings, or a number before 5.3).
fn concat2(vm: &mut Vm, a: Value, b: Value) -> Result<Value, LuaError> {
    let mut st = vec![a, b];
    vm.heap.puc_concat(&mut st, 2)?;
    Ok(st[0])
}

/// PUC `luaL_error(L, fmt, ...)`, the message made as PUC makes it from
/// its format; `conv` as for [`raise_fmt`].
pub(crate) fn raise_lib(vm: &mut Vm, fmt: &[u8], args: &[FmtArg<'_>], conv: u32) -> LuaError {
    raise(vm, Message::Fmt(fmt, args), Some(conv))
}

/// `luaL_error` with a literal message (a format with no conversions).
pub(crate) fn raise_str(vm: &mut Vm, msg: &str) -> LuaError {
    raise_bytes(vm, msg.as_bytes())
}

/// [`raise_str`] for a message PUC formats with `conv` conversions.
pub(crate) fn raise_fmt(vm: &mut Vm, conv: u32, msg: &str) -> LuaError {
    raise_bytes_fmt(vm, conv, msg.as_bytes())
}

/// 5.3+ `assert`'s default message, which `error` positions as it is.
fn raise_value(vm: &mut Vm, msg: &str) -> LuaError {
    raise(vm, Message::Pushed(msg.as_bytes()), None)
}

/// `luaL_error` with a message that need not be UTF-8.
pub(crate) fn raise_bytes(vm: &mut Vm, msg: &[u8]) -> LuaError {
    raise_bytes_fmt(vm, 0, msg)
}

/// [`raise_bytes`] for a message PUC formats with `conv` conversions.
pub(crate) fn raise_bytes_fmt(vm: &mut Vm, conv: u32, msg: &[u8]) -> LuaError {
    let (fmt, args, n) = literal(msg);
    raise(vm, Message::Fmt(fmt, &args[..n]), Some(conv))
}

/// PUC `luaL_argerror`: "bad argument #n to 'name' (extra)".
///
/// The name is the one the caller used (`lua_getinfo(L, "n")` at level 0),
/// so `local f = string.rep; f()` blames 'f'. A method call does not count
/// the self argument: a bad `#1` there becomes "calling 'm' on bad self".
/// When the caller gives no name — the native was called by another native
/// or by pcall, or through an unnamed expression — 5.2+ look the function up
/// by the name its library registered it under, and 5.1 prints '?'.
pub(crate) fn arg_error(vm: &mut Vm, n: u32, extra: &str) -> LuaError {
    // 5.5 counts the objects a `__call` chain put in front of the arguments
    // separately: an error in one of them is a "bad extra argument", and
    // the remaining arguments are numbered without them.
    let extraargs = if vm.version() >= LuaVersion::Lua55 {
        let ts = vm.thread_stack(None);
        if ts.levels.is_empty() {
            0
        } else {
            u32::try_from(vm.level_ar(&ts, 0).extraargs).expect("a __call chain length is small")
        }
    } else {
        0
    };
    let call_name = vm.running_call_name();
    let (argword, n) = if n <= extraargs {
        ("extra argument", n)
    } else {
        let n = n - extraargs;
        if let Some(("method", name)) = &call_name {
            let n = n - 1; // self is not counted
            if n == 0 {
                let args = [FmtArg::S(name.as_bytes()), FmtArg::S(extra.as_bytes())];
                return raise_lib(vm, b"calling '%s' on bad self (%s)", &args, 2);
            }
            ("argument", n)
        } else {
            ("argument", n)
        }
    };
    let name = match call_name {
        Some((_, name)) => name,
        None => match unnamed_native_name(vm) {
            Ok(name) => name,
            Err(e) => return e,
        },
    };
    let (n, name, extra) = (i64::from(n), name.as_bytes(), extra.as_bytes());
    if vm.version() >= LuaVersion::Lua55 {
        let args = [
            FmtArg::S(argword.as_bytes()),
            FmtArg::D(n),
            FmtArg::S(name),
            FmtArg::S(extra),
        ];
        return raise_lib(vm, b"bad %s #%d to '%s' (%s)", &args, 3);
    }
    let args = [FmtArg::D(n), FmtArg::S(name), FmtArg::S(extra)];
    raise_lib(vm, b"bad argument #%d to '%s' (%s)", &args, 3)
}

/// `luaL_argerror`'s fallback when `ar.name` is NULL: '?' on 5.1, otherwise
/// the running native's library name, or '?'.
///
/// 5.2 finds the name by walking the global table, whose string hashes are
/// seeded per run, so it prints `'_G.tonumber'` on some runs and `'tonumber'`
/// on others (measured on stock 5.2.4). luna gives the short form, the one
/// 5.3 settled on.
fn unnamed_native_name(vm: &mut Vm) -> Result<String, LuaError> {
    if vm.version() == crate::version::LuaVersion::Lua51 {
        return Ok("?".to_string());
    }
    let Some(target) = vm.running_natives.last().map(|a| a.nc) else {
        return Ok("?".to_string());
    };
    if vm.version() >= LuaVersion::Lua54 {
        // the function and the loaded table are pushed, then room for
        // `findfield` checked for, which a full stack refuses
        vm.native_push(2);
        let top = vm.native_top().expect("a native runs");
        // (raised with them pushed)
        argcheck::check_stack(vm, Args::new(top - 1, 0), 6, "not enough stack")?;
        vm.native_pop(2);
    }
    Ok(match vm.pushglobalfuncname(target) {
        Some(name) => {
            // the name found stays on the stack
            vm.native_push(1);
            name
        }
        None => "?".to_string(),
    })
}
