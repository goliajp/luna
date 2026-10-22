//! The package library's C functions as PUC's opening names them, and its
//! `setpath`.

use super::*;
use crate::runtime::Builtin;
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

/// luna's `loadfile` the Lua searcher loads with: its own, PUC calls
/// `luaL_loadfilex`.
fn own_loadfile(vm: &mut Vm) -> Result<Value, LuaError> {
    vm.uncounted(|vm| vm.lib_native(crate::vm::lib_os_io::nat_loadfile))
}

fn searcher_lua_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let p = package(vm, op, nup);
    let lf = op.own(vm, "loadfile", own_loadfile)?;
    op.func(vm, id, searcher_lua, Builtin::None, &[p, lf], nup)
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
/// versioned name first), else the default. luna's default Lua path is
/// its own, standing for PUC's `def` in the count, as 5.5's defaults
/// are external strings over PUC's bytes (a header each).
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
        .find_map(|n| std::env::var_os(String::from_utf8_lossy(n).as_ref()));
    let found = match found {
        Some(p) if !no_env(vm, op)? => Some(path::os_bytes(&p)),
        _ => None,
    };
    let luna_def = if field == b"path" {
        LUA_PATH_DEFAULT.to_vec()
    } else {
        cpath_default(v)
    };
    let pkg = op.get(vm, if versioned { -2 } else { -1 });
    match found {
        None => push_default(vm, op, field, &luna_def, def, pkg)?,
        Some(p) => push_env_path(vm, op, &p, &luna_def)?,
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

/// The default path pushed: luna's, counted as PUC's `def` where they
/// differ (and in 5.5 as an external string), standing on the package
/// table `pkg`.
fn push_default(
    vm: &mut Vm,
    op: &mut Opener,
    field: &[u8],
    luna: &[u8],
    def: &[u8],
    pkg: Value,
) -> Result<(), LuaError> {
    let v = vm.version();
    if v != LuaVersion::Lua55 && luna == def {
        return op.pushstring(vm, luna);
    }
    op.before(vm)?;
    let s = vm.uncounted(|vm| vm.str_value(luna));
    let s = s?;
    op.push(vm, s)?;
    // 5.1 finds the string it made before when the package table still
    // holds it (a second opening fills the same table)
    let kept = |pkg: crate::runtime::Gc<crate::runtime::Table>| {
        let k = vm.heap.find_interned(field);
        v == LuaVersion::Lua51 && k.is_some_and(|k| pkg.get(Value::Str(k)).raw_eq(s))
    };
    if let Value::Table(pkg) = pkg
        && !kept(pkg)
    {
        let size = if v == LuaVersion::Lua55 {
            vm.heap.puc_size(crate::runtime::puc::PucObj::LongStrHeader)
        } else {
            vm.heap.puc_size(crate::runtime::puc::PucObj::Str {
                len: def.len(),
                long: true,
            })
        };
        // 5.1 keeps every string in its string table; later ones stand
        // PUC's string on luna's where that is a long string of its own,
        // made now as PUC's is, so that the sweep reaches it where PUC's is
        if v == LuaVersion::Lua51 {
            vm.heap.acct_stand_strt(pkg.header(), def.len());
        } else {
            match s {
                Value::Str(ls)
                    if luna.len() > vm.heap.puc.short_max
                        && crate::runtime::heap::is_uncounted(ls.header()) =>
                {
                    vm.heap.acct_stand_on(ls.header(), size, true)
                }
                // kept where PUC's list has it, as long as the package table
                _ => {
                    let host = pkg.header();
                    vm.retry_oom(|vm| vm.heap.acct_kept_string(host, size))?
                }
            }
        }
    }
    op.after(vm)
}

/// A path from the environment pushed with the default in place of its
/// ";;": 5.1 to 5.3 through `luaL_gsub` twice (";;" marked, the mark
/// replaced), the first result dropped; later in one buffer.
fn push_env_path(vm: &mut Vm, op: &mut Opener, p: &[u8], def: &[u8]) -> Result<(), LuaError> {
    if vm.version() <= LuaVersion::Lua53 {
        let marked = path::replace(p, b";;", b";\x01;");
        op.pushstring(vm, &marked)?;
        op.pushstring(vm, &path::replace(&marked, b"\x01", def))?;
        op.remove(vm, -2);
        return Ok(());
    }
    op.pushstring(vm, &path::splice_default(p, def))
}
