//! The package library's C functions as PUC's opening names them, and its
//! `setpath`.

use super::progdir;
use super::*;
use crate::runtime::Builtin;
use crate::runtime::puc::records::strcache::{CStr, site};
use crate::vm::opening::{Ctor, Natives, Opener, run_open};
use Ctor::{C, F};

pub(crate) const PUC_FNS: Natives = &[
    ("gctm", F(clibs_gc, Builtin::None)),
    ("ll_loadlib", F(ll_loadlib, Builtin::None)),
    ("ll_module", C(module_fn)),
    ("ll_require", C(require_fn)),
    ("ll_searchpath", F(ll_searchpath, Builtin::None)),
    ("ll_seeall", F(ll_seeall, Builtin::None)),
    ("loader_C", C(searcher_c_fn)),
    ("loader_Croot", C(searcher_croot_fn)),
    ("loader_Lua", C(searcher_lua_fn)),
    ("loader_preload", C(searcher_preload_fn)),
    ("luaopen_package", Ctor::Open(luaopen_package)),
    ("searcher_C", C(searcher_c_fn)),
    ("searcher_Croot", C(searcher_croot_fn)),
    ("searcher_Lua", C(searcher_lua_fn)),
    ("searcher_preload", C(searcher_preload_fn)),
];

fn luaopen_package(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_package", 1)
}

/// The package table a function of the library keeps: PUC's upvalue, or
/// in 5.1 its environment.
fn package(vm: &Vm, op: &Opener, nup: u32) -> Value {
    if nup > 0 {
        op.upval(vm, 0, nup)
    } else {
        op.env_value(vm)
    }
}

/// The registry's `_LOADED`, which `require` and `module` keep.
fn loaded(vm: &Vm) -> Value {
    let reg = vm.registry.map_or(Value::Nil, Value::Table);
    match (reg, vm.heap.find_interned(b"_LOADED")) {
        (Value::Table(r), Some(k)) => r.get(Value::Str(k)),
        _ => Value::Nil,
    }
}

fn searcher_preload_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let p = package(vm, op, nup);
    op.func(vm, id, searcher_preload, Builtin::None, &[p], nup)
}

fn searcher_c_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let p = package(vm, op, nup);
    op.func(vm, id, searcher_c, Builtin::None, &[p], nup)
}

fn searcher_croot_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let p = package(vm, op, nup);
    op.func(vm, id, searcher_croot, Builtin::None, &[p], nup)
}

fn searcher_lua_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let p = package(vm, op, nup);
    op.func(vm, id, searcher_lua, Builtin::None, &[p], nup)
}

/// The mark of a module being loaded, to catch loops: a light userdata
/// of PUC's, nothing it counts.
fn sentinel(vm: &mut Vm) -> Result<Value, LuaError> {
    let s = vm.uncounted(|vm| vm.lib_userdata(UserdataPayload::Empty, false, 0));
    Ok(Value::Userdata(s?))
}

fn require_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let p = package(vm, op, nup);
    let s = op.own(vm, "sentinel", sentinel)?;
    let up = [p, loaded(vm), s];
    op.func(vm, id, ll_require, Builtin::None, &up, nup)
}

fn module_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    op.func(vm, id, ll_module, Builtin::None, &[loaded(vm)], nup)
}

/// PUC's `setpath`: the field `field` of the package table (below the
/// top) set to the environment's path (`env`, then `env2`; 5.3 on the
/// versioned name first), else the default `def` (on Windows its own),
/// 5.5's an external string over the default's bytes.
pub(crate) fn puc_setpath(
    vm: &mut Vm,
    op: &mut Opener,
    field: &[u8],
    env: Option<&[u8]>,
    env2: Option<&[u8]>,
    def: &[u8],
) -> Result<(), LuaError> {
    let v = vm.version();
    let env = env.unwrap_or_default();
    let versioned = v >= LuaVersion::Lua53;
    let mut names = vec![env.to_vec()];
    if versioned {
        let suffix = match v {
            LuaVersion::Lua53 => "_5_3",
            LuaVersion::Lua55 => "_5_5",
            _ => "_5_4",
        };
        let nver = [env, suffix.as_bytes()].concat();
        op.before(vm)?;
        if v == LuaVersion::Lua53 {
            // `luaO_pushvfstring` pushes each piece, then concatenates
            for piece in [&b""[..], env, suffix.as_bytes()] {
                vm.str_value(piece)?;
            }
        }
        let s = vm.str_value(&nver)?;
        op.push(vm, s)?;
        op.after(vm)?;
        names.insert(0, nver);
    }
    if let Some(e2) = env2 {
        names.push(e2.to_vec());
    }
    let found = names
        .iter()
        .find_map(|n| std::env::var_os(String::from_utf8_lossy(n).as_ref()).map(|p| (n, p)));
    let found = match found {
        Some((n, p)) if !no_env(vm, op)? => Some((n.clone(), path::os_bytes(&p))),
        _ => None,
    };
    let def = if cfg!(windows) {
        progdir::windows_default(v, field)
    } else {
        def.to_vec()
    };
    match found {
        None => push_default(vm, op, field, &def, v == LuaVersion::Lua55)?,
        Some((name, p)) => push_env_path(vm, op, &name, &p, &def)?,
    }
    // PUC's default names the executable's directory as `!`, which
    // `setprogdir`'s copy replaces (the original left garbage)
    if cfg!(windows) {
        progdir::setprogdir(vm, op)?;
    }
    let ix = op.ix(vm, if versioned { -3 } else { -2 });
    op.setfield(vm, ix, field)?;
    if versioned {
        op.pop(vm, 1);
    }
    Ok(())
}

/// lua.c's `-E` (`noenv`): the registry's `LUA_NOENV`, read from 5.2 on
/// (luna's 5.1 has the switch too, with nothing to read).
fn no_env(vm: &mut Vm, op: &mut Opener) -> Result<bool, LuaError> {
    if vm.version() == LuaVersion::Lua51 {
        return Ok(vm.ignore_env);
    }
    op.getfield(vm, crate::vm::opening::Ix::Registry, b"LUA_NOENV")?;
    let b = op.get(vm, -1).truthy();
    op.pop(vm, 1);
    Ok(b)
}

/// The default path `def` of `field` pushed as PUC pushes it: with
/// `lua_pushstring`, through `luaS_new`'s cache at the default's address;
/// 5.5's with `lua_pushexternalstring` and no `falloc`, a long string over
/// bytes PUC does not own (every default is longer than a short string).
fn push_default(
    vm: &mut Vm,
    op: &mut Opener,
    field: &[u8],
    def: &[u8],
    external: bool,
) -> Result<(), LuaError> {
    if !external {
        let at = CStr::Fixed(site::LITERAL, site::name_hash(field));
        return push_at(vm, op, at, def);
    }
    debug_assert!(def.len() > crate::runtime::string::MAX_SHORT_LEN);
    op.before(vm)?;
    let s = vm.retry_oom(|vm| vm.heap.new_external_string(def, true))?;
    op.push(vm, Value::Str(s))?;
    op.after(vm)
}

/// `lua_pushstring` of the C string at `at` holding `bytes`.
fn push_at(vm: &mut Vm, op: &mut Opener, at: CStr, bytes: &[u8]) -> Result<(), LuaError> {
    op.before(vm)?;
    let s = vm.str_value_at(at, bytes)?;
    op.push(vm, s)?;
    op.after(vm)
}

/// The path `p` of the environment's variable `name` pushed with the
/// default in place of its ";;": 5.1 to 5.3 through `luaL_gsub` twice
/// (";;" marked, the mark replaced), the first result dropped; later in
/// one buffer, or, with no ";;", the variable's own string through
/// `luaS_new`'s cache.
fn push_env_path(
    vm: &mut Vm,
    op: &mut Opener,
    name: &[u8],
    p: &[u8],
    def: &[u8],
) -> Result<(), LuaError> {
    if vm.version() <= LuaVersion::Lua53 {
        let marked = path::replace(p, b";;", b";\x01;");
        op.pushstring(vm, &marked)?;
        op.pushstring(vm, &path::replace(&marked, b"\x01", def))?;
        op.remove(vm, -2);
        return Ok(());
    }
    if !p.windows(2).any(|w| w == b";;") {
        return push_at(vm, op, CStr::Fixed(site::GETENV, site::name_hash(name)), p);
    }
    op.pushstring(vm, &path::splice_default(p, def))
}
