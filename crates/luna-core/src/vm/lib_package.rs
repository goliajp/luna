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
mod path;
mod progdir;
mod require;
mod search_acct;
mod searchers;

use module51::*;
use path::*;
use require::*;
use searchers::*;
mod puc_fns;
pub(crate) use puc_fns::{PUC_FNS, puc_setpath};

/// PUC's `LUA_DIRSEP`: what `require` and `package.searchpath` turn a
/// module name's dots into, and the first line of `package.config`.
const LUA_DIRSEP: &[u8] = if cfg!(windows) { b"\\" } else { b"/" };

/// A string of the opening script, recorded from PUC built for Linux, as
/// PUC built for the target has it: Windows' `package.config` starts with
/// its `LUA_DIRSEP`.
#[inline]
pub(crate) fn opening_str(s: &'static [u8]) -> &'static [u8] {
    if !cfg!(windows) {
        return s;
    }
    match s {
        b"/\n;\n?\n!\n-" => b"\\\n;\n?\n!\n-",
        b"/\n;\n?\n!\n-\n" => b"\\\n;\n?\n!\n-\n",
        _ => s,
    }
}

/// PUC's message and `loadlib` "where" when built without dynamic libraries.
const DLMSG: &[u8] = b"dynamic libraries not enabled; check your Lua installation";

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

/// `luaL_getsubtable(L, LUA_REGISTRYINDEX, name)`: the registry's table
/// `name`, made on first use.
fn registry_table(vm: &mut Vm, name: &str) -> Result<Gc<Table>, LuaError> {
    let reg = vm.host_registry();
    let k = vm.str_value(name.as_bytes())?;
    if let Value::Table(t) = reg.get(k) {
        return Ok(t);
    }
    let t = vm.alloc_table()?;
    raw_set_checked(vm, reg, k, Value::Table(t))?;
    Ok(t)
}

/// The C libraries' finalizer: luna loads none.
pub(crate) fn clibs_gc(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(0)
}
