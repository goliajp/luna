//! `luaL_argerror`: the message a bad argument raises.

use super::*;
use crate::runtime::puc::fmt::FmtArg;

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
/// on others; luna walks it in the same order, so with the same seed it
/// prints what PUC does.
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
    Ok(match vm.pushglobalfuncname(target)? {
        Some(name) => {
            // the name found stays on the stack
            vm.native_push(1);
            name
        }
        None => "?".to_string(),
    })
}
