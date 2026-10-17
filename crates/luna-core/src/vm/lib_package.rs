//! package library: `require` and the searchers it runs, `package.path` /
//! `cpath` / `config` / `loaded` / `preload`, `package.searchpath`,
//! `package.loadlib`, and 5.1/5.2's `module` / `package.seeall`. Shaped per
//! dialect after loadlib.c 5.1–5.5 (5.2 with LUA_COMPAT_MODULE and
//! LUA_COMPAT_LOADERS, as its default build has them).
//!
//! luna links no dynamic loader: `package.loadlib` and the C searchers fail
//! the way a PUC build without dynamic-library support does.

use crate::runtime::{Gc, Table, TableError, UserdataPayload, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::raise_str;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::lib_io::c_str;

mod module51;
mod open;
pub(crate) use open::{clibs_gc, clibs_metatable};
mod path;
mod require;
mod searchers;

use module51::*;
use path::*;
use require::*;
use searchers::*;

/// Default Lua search path. PUC compiles in its install prefix; luna has
/// none, so only the current-directory templates remain.
const LUA_PATH_DEFAULT: &[u8] = b"./?.lua;./?/init.lua";

/// PUC's message and `loadlib` "where" when built without dynamic libraries.
const DLMSG: &[u8] = b"dynamic libraries not enabled; check your Lua installation";

/// [`open_package_bare`], raising a memory error.
pub(crate) fn try_open_package_bare(vm: &mut Vm) -> Result<(), LuaError> {
    let v = vm.version();
    if v >= LuaVersion::Lua52 {
        return open::open_package_puc(vm);
    }
    // 5.1's `luaopen_package`: the package table its functions are
    // registered in (`luaL_register`, which sets the global), the fields
    // in its order, then `module` and `require` among the globals
    let pkg = vm.lib_table_puc("package")?;
    vm.lib_set_global("package", Value::Table(pkg))?;
    let loaded = registry_table(vm, "_LOADED")?;
    let loaders = vm.lib_table_puc("package.loaders")?;
    let lf = vm.lib_native(crate::vm::lib_os_io::nat_loadfile)?;
    let fns = [
        vm.lib_native_nup(searcher_preload, &[Value::Table(pkg)], 0)?,
        vm.lib_native_nup(searcher_lua, &[Value::Table(pkg), lf], 0)?,
        vm.lib_native_nup(searcher_c, &[Value::Table(pkg)], 0)?,
        vm.lib_native_nup(searcher_croot, &[Value::Table(pkg)], 0)?,
    ];
    for (i, f) in fns.into_iter().enumerate() {
        vm.lib_set_value(loaders, Value::Int(i as i64 + 1), f)?;
    }
    vm.barrier_back_table(loaders);
    let path = env_path(v, false, "LUA_PATH", LUA_PATH_DEFAULT);
    let cpath = env_path(v, false, "LUA_CPATH", &cpath_default(v));
    let entries = vec![
        ("loadlib", vm.lib_native(ll_loadlib)?),
        ("seeall", vm.lib_native(ll_seeall)?),
        ("loaders", Value::Table(loaders)),
        ("path", vm.str_value(&path)?),
        ("cpath", vm.str_value(&cpath)?),
        ("config", vm.str_value(b"/\n;\n?\n!\n-")?),
        ("loaded", Value::Table(loaded)),
        ("preload", Value::Table(vm.alloc_table()?)),
    ];
    vm.lib_put_all(pkg, "package", entries)?;
    // a module being loaded is marked with a sentinel, to catch loops (a
    // light userdata of PUC's: nothing it counts)
    let sentinel = Value::Userdata(vm.lib_userdata(UserdataPayload::Empty, false, 0)?);
    let m = vm.lib_native_nup(ll_module, &[Value::Table(loaded)], 0)?;
    vm.lib_set_global("module", m)?;
    let req = vm.lib_native_nup(
        ll_require,
        &[Value::Table(pkg), Value::Table(loaded), sentinel],
        0,
    )?;
    vm.lib_set_global("require", req)?;
    vm.barrier_back_table(pkg);
    vm.barrier_back_table(loaded);
    Ok(())
}

impl Vm {
    /// lua.c's `-E`: the package library opened after this takes its
    /// default `path` and `cpath` instead of reading `LUA_PATH` /
    /// `LUA_CPATH` (5.2 on; 5.1 has no such switch), and a registry the
    /// debug library makes after it holds `LUA_NOENV = true`, as lua.c sets
    /// it for the libraries.
    pub fn set_ignore_env(&mut self, ignore: bool) {
        self.ignore_env = ignore;
    }
}

fn raw_set(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) -> Result<(), LuaError> {
    let k = vm.str_value(k.as_bytes())?;
    vm.lib_set_value(t, k, v)?;
    Ok(())
}

/// A raw store into a table a script can reach and fill: a full hash
/// part raises "table overflow" as any other store does.
fn raw_set_checked(vm: &mut Vm, t: Gc<Table>, k: Value, v: Value) -> Result<(), LuaError> {
    // SAFETY: `t` is a table the caller holds (the package table or one reached from it); no reference into it is live across the `set`, which does not collect
    match vm.table_op(|vm| unsafe { t.as_mut() }.set(&mut vm.heap, k, v)) {
        Ok(()) => {}
        Err(e @ (TableError::ReadOnly | TableError::Memory(_))) => return Err(vm.table_error(e)),
        Err(_) => return Err(vm.rt_err("table overflow")),
    }
    vm.barrier_back_table(t);
    Ok(())
}

/// `luaL_getsubtable(L, LUA_REGISTRYINDEX, name)`: the registry's table
/// `name`, created on first use. Without a registry (the debug library
/// makes it) the table is simply not reachable from there.
fn registry_table(vm: &mut Vm, name: &str) -> Result<Gc<Table>, LuaError> {
    let Some(reg) = vm.registry else {
        return vm.alloc_table();
    };
    let k = vm.str_value(name.as_bytes())?;
    if let Value::Table(t) = reg.get(k) {
        return Ok(t);
    }
    let t = vm.alloc_table()?;
    raw_set_checked(vm, reg, k, Value::Table(t))?;
    Ok(t)
}

fn upval_table(vm: &Vm, fs: u32, i: usize) -> Gc<Table> {
    match vm.nat_upval(fs, i) {
        Value::Table(t) => t,
        _ => unreachable!("package natives keep tables in their upvalues"),
    }
}

/// Whether `a` is the sentinel userdata `b`.
fn same(a: Value, b: Value) -> bool {
    matches!((a, b), (Value::Userdata(x), Value::Userdata(y)) if x.ptr_eq(y))
}

fn str_value(vm: &mut Vm, b: &[u8]) -> Result<Value, LuaError> {
    vm.str_value(b)
}
