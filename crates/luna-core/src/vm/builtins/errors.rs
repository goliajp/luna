//! `assert`, `error` and the `luaL_error` / `luaL_argerror` helpers the
//! library modules share.

use super::*;

pub(super) fn nat_assert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = a.get(vm, 0);
    if v.truthy() {
        // assert returns all its arguments
        let vals: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
        return Ok(vm.nat_return(fs, &vals));
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
                Some(msg) => Err(raise(vm, Value::Str(msg))),
                None => Err(raise_str(vm, "assertion failed!")),
            }
        }
        // 5.3+ hands the message, of any type, to `error` at level 1; an
        // explicit nil message stays nil (`lua_settop(L, 1)` keeps it).
        _ => {
            argcheck::check_any(vm, a, 0)?;
            if nargs >= 2 {
                Err(raise(vm, a.get(vm, 1)))
            } else {
                Err(raise_str(vm, "assertion failed!"))
            }
        }
    }
}

pub(super) fn nat_error(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    if level <= 0 {
        return Err(LuaError(msg));
    }
    // ≤5.2 tests `lua_isstring`, so a number message is positioned too and
    // comes out as a string; 5.3+ positions only real strings.
    let text = match msg {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(_) | Value::Float(_) if vm.version() <= LuaVersion::Lua52 => {
            argcheck::to_str_bytes(vm, msg).expect("a number converts to a string")
        }
        v => return Err(LuaError(v)),
    };
    // PUC `luaB_error` calls `luaL_where(L, level)` — prepend the position of
    // the Lua frame `level` steps up. If the level is out of range or the
    // target frame has no line info, fall through with no prefix.
    let mut out = vm
        .position_prefix_at_level(level as i64)
        .map(String::into_bytes)
        .unwrap_or_default();
    out.extend_from_slice(&text);
    Err(LuaError(Value::Str(vm.heap.intern(&out))))
}

/// Raise a string-ish error with the caller's position prefix (PUC level 1).
/// PUC `luaL_error`: a string message gets `luaL_where(L, 1)`, the position
/// of whatever called the running native — including a Lua frame that
/// reached it as a metamethod — and nothing when that caller is C.
fn raise(vm: &mut Vm, msg: Value) -> LuaError {
    match msg {
        Value::Str(s) => {
            let text = match vm.position_prefix_at_level(1) {
                Some(p) => {
                    let mut t = p.into_bytes();
                    t.extend_from_slice(s.as_bytes());
                    t
                }
                None => s.as_bytes().to_vec(),
            };
            LuaError(Value::Str(vm.heap.intern(&text)))
        }
        v => LuaError(v),
    }
}

pub(crate) fn raise_str(vm: &mut Vm, msg: &str) -> LuaError {
    raise_bytes(vm, msg.as_bytes())
}

/// `luaL_error` with a message that need not be UTF-8.
pub(crate) fn raise_bytes(vm: &mut Vm, msg: &[u8]) -> LuaError {
    let s = Value::Str(vm.heap.intern(msg));
    raise(vm, s)
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
                return raise_str(vm, &format!("calling '{name}' on bad self ({extra})"));
            }
            ("argument", n)
        } else {
            ("argument", n)
        }
    };
    let name = match call_name {
        Some((_, name)) => name,
        None => unnamed_native_name(vm),
    };
    raise_str(vm, &format!("bad {argword} #{n} to '{name}' ({extra})"))
}

/// `luaL_argerror`'s fallback when `ar.name` is NULL: '?' on 5.1, otherwise
/// the running native's library name, or '?'.
///
/// 5.2 finds the name by walking the global table, whose string hashes are
/// seeded per run, so it prints `'_G.tonumber'` on some runs and `'tonumber'`
/// on others (measured on stock 5.2.4). luna gives the short form, the one
/// 5.3 settled on.
fn unnamed_native_name(vm: &mut Vm) -> String {
    if vm.version() == crate::version::LuaVersion::Lua51 {
        return "?".to_string();
    }
    let Some(target) = vm.running_natives.last().map(|a| a.nc.f) else {
        return "?".to_string();
    };
    vm.pushglobalfuncname(target)
        .unwrap_or_else(|| "?".to_string())
}
