//! `assert`, `error` and the `luaL_error` / `luaL_argerror` helpers the
//! library modules share.

use super::*;
use crate::runtime::puc::fmt::FmtArg;
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
            // the default message is pushed whether or not a message is
            // there to take its place (`lua_pushliteral` before `lua_settop`)
            if nargs >= 2 {
                vm.api_push_string_dyn(b"assertion failed!")?;
            }
            vm.native_settop(1);
            match a.get(vm, 1) {
                _ if nargs < 2 => Err(raise_value(vm, "assertion failed!")),
                Value::Str(s) => Err(raise(vm, Message::Pushed(s.as_bytes()), None)),
                v => Err(LuaError(v)),
            }
        }
    }
}

pub(crate) fn nat_error<const PRE: bool>(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
) -> Result<u32, LuaError> {
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
    let base = vm.lib_roots.mark();
    let r = vm
        .api_pushing::<PRE>(|vm| lual_where(vm, pos))
        .and_then(|at| {
            vm.api_check_before::<PRE>()?;
            let v = concat2(vm, Value::Str(at), msg)?;
            vm.api_check_after::<PRE>(v)?;
            Ok(v)
        });
    vm.lib_roots.release(base);
    Err(LuaError(r?))
}

/// Raise a string-ish error with the caller's position prefix (PUC level 1).
/// PUC `luaL_error`: a string message gets `luaL_where(L, 1)`, the position
/// of whatever called the running native — including a Lua frame that
/// reached it as a metamethod — and nothing when that caller is C. `conv`:
/// the conversions (`%s`, `%d`, ...) of the format PUC formats the message
/// with, whose pieces it pushes one by one (see [`Vm::format_pushes`]);
/// `None`: the message is pushed as it is (`error` and 5.3+ `assert`).
fn raise(vm: &mut Vm, msg: Message<'_>, conv: Option<u32>) -> LuaError {
    with_rules!(vm, P => raise_by::<P>(vm, msg, conv))
}

fn raise_by<const PRE: bool>(vm: &mut Vm, msg: Message<'_>, conv: Option<u32>) -> LuaError {
    let pos = vm.position_at_level(1);
    // `luaL_where`, a formatted position or a literal "", then the message
    let at = pos.is_some().then_some(2);
    if let Some(e) = vm.format_pushes(&[at, conv]) {
        return e;
    }
    // concatenated into one value; each piece is pushed, as the API calls
    // that make them check for a collection
    vm.native_push(1);
    let base = vm.lib_roots.mark();
    let r = (|| {
        // 5.3+ `assert` pushes its message before `luaB_error` positions it
        let early = match msg {
            Message::Pushed(m) => Some(vm.api_pushing::<PRE>(|vm| vm.alloc_str(m))?),
            Message::Fmt(..) => None,
        };
        let at = vm.api_pushing::<PRE>(|vm| lual_where(vm, pos))?;
        let m = match (early, msg) {
            (Some(m), _) => m,
            (None, Message::Fmt(fmt, args)) => {
                vm.api_pushing::<PRE>(|vm| vm.pushfstring(fmt, args))?
            }
            (None, Message::Pushed(_)) => unreachable!("pushed above"),
        };
        vm.api_check_before::<PRE>()?;
        let v = concat2(vm, Value::Str(at), Value::Str(m))?;
        vm.api_check_after::<PRE>(v)?;
        Ok(v)
    })();
    vm.lib_roots.release(base);
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

/// PUC `luaL_where`: the string of position `pos` (`"%s:%d: "`), `""`
/// without one.
pub(crate) fn lual_where(vm: &mut Vm, pos: Option<(Vec<u8>, i64)>) -> Result<Gc<LuaStr>, LuaError> {
    match pos {
        Some((src, line)) => vm.pushfstring(b"%s:%d: ", &[FmtArg::S(&src), FmtArg::D(line)]),
        None => vm.alloc_str(b""),
    }
}

/// `lua_concat(L, 2)` of `a` and `b` (strings, or a number before 5.3).
pub(crate) fn concat2(vm: &mut Vm, a: Value, b: Value) -> Result<Value, LuaError> {
    let mut st = vec![a, b];
    vm.concat_pieces(&mut st, 2)?;
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

/// [`raise_bytes`] for a message PUC formats with `conv` conversions: the
/// text is PUC's format, each '%' in it a "%%" (a piece of its own
/// before 5.4).
pub(crate) fn raise_bytes_fmt(vm: &mut Vm, conv: u32, msg: &[u8]) -> LuaError {
    let mut fmt = Vec::with_capacity(msg.len());
    for &c in msg {
        fmt.push(c);
        if c == b'%' {
            fmt.push(b'%');
        }
    }
    raise(vm, Message::Fmt(&fmt, &[]), Some(conv))
}
