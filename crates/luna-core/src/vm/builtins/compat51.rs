//! 5.1-only base functions: `newproxy`, `gcinfo`, `setfenv`, `getfenv`.

use super::*;

/// PUC 5.1 `newproxy([false | true | proxy])`: an empty userdata, with no
/// metatable, a fresh one, or the metatable of another proxy. Only a
/// userdata whose metatable `newproxy(true)` created — recorded in the weak
/// set held as upvalue 0 — is a proxy; anything else is an argument error.
pub(super) fn nat_newproxy(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::userdata::UserdataPayload;
    let Value::Table(proxies) = vm.nat_upval(fs, 0) else {
        unreachable!("newproxy is registered with its metatable set")
    };
    let arg = vm.nat_arg(fs, nargs, 0);
    let mt = match arg {
        v if !v.truthy() => None,
        Value::Bool(true) => {
            let m = vm.heap.new_table();
            // SAFETY: `proxies` is the weak set newproxy carries as its upvalue, so the running native keeps it alive; no reference into it is live across the `set`, which does not collect
            unsafe { proxies.as_mut() }
                .set(&mut vm.heap, Value::Table(m), Value::Bool(true))
                .expect("a table key is never nil or NaN");
            vm.barrier_back_table(proxies);
            Some(m)
        }
        v => match vm.metatable_of(v) {
            Some(m) if proxies.get(Value::Table(m)).truthy() => Some(m),
            _ => return Err(arg_error(vm, 1, "boolean or proxy expected")),
        },
    };
    let u = vm.heap.new_userdata(UserdataPayload::Empty, false);
    if let Some(mt) = mt {
        // SAFETY: `u` was allocated on the line above and is held only by this local; the borrow covers one call
        unsafe { u.as_mut() }.set_metatable(Some(mt));
        // PUC 5.1 registered *every* userdata with a metatable for
        // finalization (`luaC_checkfinalizer` deferred the `__gc` check to
        // GC time, so adding `__gc` to a shared metatable *after* the
        // proxy was made still works). 5.2+ moved the check to
        // setmetatable time; that's gated by version when needed.
        vm.heap.register_finalizable_userdata(u);
    }
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// PUC 5.1 `gcinfo()` — memory in use, in KB (an integer in PUC, which had
/// no integer subtype yet; luna mirrors the rounding). Replaced in 5.2+ by
/// `collectgarbage("count")`. gc.lua 5.1 :88 uses it as a loop guard.
pub(super) fn nat_gcinfo(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let kb = (vm.gc_count_bytes() as f64 / 1024.0).floor() as i64;
    Ok(vm.nat_return(fs, &[Value::Int(kb)]))
}

/// What PUC 5.1's `getfunc` finds for argument 1 of `getfenv`/`setfenv`.
enum FenvTarget {
    /// A Lua function: given directly, or running at the level.
    Lua(crate::runtime::Gc<crate::runtime::LuaClosure>),
    /// A C function, given directly or running at the level (level 0 is
    /// the calling `getfenv`/`setfenv` itself).
    C,
}

/// PUC 5.1 `getfunc`: argument 1 is a function, or a stack level — optional
/// (default 1) for `getfenv`, required for `setfenv`.
fn fenv_target(vm: &mut Vm, a: Args, level_optional: bool) -> Result<FenvTarget, LuaError> {
    match a.get(vm, 0) {
        Value::Closure(c) => return Ok(FenvTarget::Lua(c)),
        Value::Native(_) => return Ok(FenvTarget::C),
        _ => {}
    }
    let level = if level_optional {
        argcheck::opt_int(vm, a, 0, 1)?
    } else {
        argcheck::check_int(vm, a, 0)?
    };
    if level < 0 {
        return Err(arg_error(vm, 1, "level must be non-negative"));
    }
    if level == 0 {
        return Ok(FenvTarget::C);
    }
    use crate::vm::callstack::DbgKind;
    match vm.dbg_frame(level as i64) {
        Some(DbgKind::Lua(_)) => Ok(FenvTarget::Lua(
            vm.lua_closure_at_level(level as i64)
                .expect("a Lua level has a closure"),
        )),
        Some(DbgKind::C(_)) => Ok(FenvTarget::C),
        Some(DbgKind::Tail) => Err(raise_str(
            vm,
            &format!("no function environment for tail call at level {level}"),
        )),
        None => Err(arg_error(vm, 1, "invalid level")),
    }
}

/// The index of the `_ENV` upvalue of a 5.1 closure. It is *not* guaranteed
/// to sit at slot 0 — closures capture upvalues in first-access order, so a
/// body that touches a local upvalue (e.g. `local saved = print;
/// saved("hi"); module(...)`) puts the local ahead of `_ENV`.
fn env_upvalue(cl: crate::runtime::Gc<crate::runtime::LuaClosure>) -> Option<usize> {
    cl.proto.upvals.iter().position(|d| &*d.name == "_ENV")
}

/// PUC 5.1 `setfenv(f|level, env)`: replace the env of the Lua function `f`
/// (or of the Lua function at stack `level`). The closure gets a new `_ENV`
/// cell, so the change does not reach the functions that shared the old one.
pub(super) fn nat_setfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let env = argcheck::check_table(vm, a, 1)?;
    let target = fenv_target(vm, a, false)?;
    // `setfenv(0, env)` (a numeric 0, string "0" included) rewrites the
    // thread's global table (`L->l_gt`). luna repoints `Vm.globals` so future
    // `vm.load` calls snapshot the new table into the loaded chunk's `_ENV`
    // cell; already-loaded closures keep their own per-closure `_ENV` cell.
    // locals.lua's `foo("")` probe relies on the loaded-chunk path picking up
    // the new table.
    if argcheck::to_num(vm, a.get(vm, 0)).is_some_and(|n| n.as_f64() == 0.0) {
        vm.set_globals(env);
        return Ok(vm.nat_return(fs, &[]));
    }
    let cl = match target {
        FenvTarget::Lua(cl) => cl,
        FenvTarget::C => {
            return Err(raise_str(
                vm,
                "'setfenv' cannot change environment of given object",
            ));
        }
    };
    let env_idx = env_upvalue(cl)
        .ok_or_else(|| raise_str(vm, "'setfenv' cannot change environment of given object"))?;
    vm.set_closure_env(cl, env_idx, env);
    Ok(vm.nat_return(fs, &[Value::Closure(cl)]))
}

/// PUC 5.1 `getfenv(f|level)`: the env of the Lua function `f` (or of the
/// Lua function at stack `level`); for a C function, the thread's globals.
pub(super) fn nat_getfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::UpvalState;
    let env = match fenv_target(vm, Args::new(fs, nargs), true)? {
        FenvTarget::Lua(c) => match env_upvalue(c) {
            Some(i) => match c.upvals()[i].state() {
                UpvalState::Closed(v) => v,
                UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
            },
            None => Value::Table(vm.globals()),
        },
        FenvTarget::C => Value::Table(vm.globals()),
    };
    Ok(vm.nat_return(fs, &[env]))
}
