//! Minimal base library. The full base library replaces/extends this.

use crate::runtime::{Builtin, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// Open the base library, failing when memory runs out; without
/// `loadfile` and `dofile`, which luna opens with `os` and `io`.
pub(crate) fn try_open_base(vm: &mut Vm) -> Result<(), LuaError> {
    open_base(vm, false)
}

/// PUC's base library, which has `loadfile` and `dofile`.
pub(crate) fn try_open_base_puc(vm: &mut Vm) -> Result<(), LuaError> {
    open_base(vm, true)
}

fn open_base(vm: &mut Vm, loaders: bool) -> Result<(), LuaError> {
    use LuaVersion as V;
    let v = vm.version();
    let globals = Value::Table(vm.globals());
    // the globals in the order PUC's base library makes and puts them:
    // `base_funcs`, with `_G` before them up to 5.2 and after from 5.3 on;
    // 5.1's `auxopen` makes `ipairs` and `pairs` last
    let mut g: Vec<(&str, Value)> = Vec::with_capacity(32);
    if v <= V::Lua52 {
        g.push(("_G", globals));
    }
    g.push(("assert", vm.lib_native(nat_assert)?));
    let f = vm.lib_native(crate::vm::lib_gc::nat_collectgarbage)?;
    g.push(("collectgarbage", f));
    if loaders {
        g.push(("dofile", vm.lib_native(crate::vm::lib_os_io::nat_dofile)?));
    }
    g.push(("error", vm.builtin(nat_error, &[], Builtin::Error)?));
    if v == V::Lua51 {
        // PUC 5.1 also exposed `gcinfo()` (memory in KB); `setfenv` and
        // `getfenv`: every Lua function carries its own env (5.1
        // `LClosure.env`), retired in 5.2 for the `_ENV` upvalue. A 5.1
        // closure shares its creator's `_ENV` cell and `setfenv` replaces
        // the cell, so the change only affects that closure
        g.push(("gcinfo", vm.lib_native(nat_gcinfo)?));
        g.push(("getfenv", vm.lib_native(nat_getfenv)?));
    }
    g.push(("getmetatable", vm.lib_native(nat_getmetatable)?));
    if v >= V::Lua52 {
        g.push(("ipairs", ipairs(vm)?));
    }
    if loaders {
        let f = vm.lib_native(crate::vm::lib_os_io::nat_loadfile)?;
        g.push(("loadfile", f));
    }
    let load_obj = vm.lib_native(nat_load)?;
    g.push(("load", load_obj));
    // 5.1 keeps `loadstring`; PUC 5.2's official build ships
    // -DLUA_COMPAT_ALL, so it survives there as a `load` alias
    match v {
        V::Lua51 => g.push(("loadstring", vm.lib_native(nat_loadstring)?)),
        V::Lua52 => g.push(("loadstring", load_obj)),
        _ => {}
    }
    let next_obj = vm.lib_native(nat_next)?;
    g.push(("next", next_obj));
    // 5.2+ pairs returns the global next itself (a light C function, equal
    // to every other push of `luaB_next`)
    if v >= V::Lua52 {
        g.push(("pairs", vm.builtin(nat_pairs, &[next_obj], Builtin::Pairs)?));
    }
    g.push(("pcall", vm.builtin(nat_pcall, &[], Builtin::Pcall)?));
    g.push(("print", vm.lib_native(nat_print)?));
    // PUC 5.4 introduced the warning system. `warn(msg1, …, msgN)` emits
    // pieces of one message via the default warnf (`lauxlib.c::warnfon/off`),
    // which recognises `@on` / `@off` control messages and starts disabled.
    if v >= V::Lua54 {
        g.push(("warn", vm.lib_native(nat_warn)?));
    }
    g.push(("rawequal", vm.lib_native(nat_rawequal)?));
    if v >= V::Lua52 {
        g.push(("rawlen", vm.lib_native(nat_rawlen)?));
    }
    g.push(("rawget", vm.lib_native(nat_rawget)?));
    g.push(("rawset", vm.lib_native(nat_rawset)?));
    g.push(("select", vm.lib_native(nat_select)?));
    if v == V::Lua51 {
        g.push(("setfenv", vm.lib_native(nat_setfenv)?));
    }
    g.push(("setmetatable", vm.lib_native(nat_setmetatable)?));
    g.push(("tonumber", vm.lib_native(nat_tonumber)?));
    g.push((
        "tostring",
        vm.builtin(nat_tostring, &[], Builtin::Tostring)?,
    ));
    g.push(("type", vm.lib_native(nat_type)?));
    if v == V::Lua51 {
        // 5.2 moved it to `table.unpack`
        let f = vm.lib_native(crate::vm::lib_table::t_unpack)?;
        g.push(("unpack", f));
    }
    g.push(("xpcall", vm.builtin(nat_xpcall, &[], Builtin::Xpcall)?));
    if v >= V::Lua53 {
        g.push(("_G", globals));
    }
    let version = match v {
        V::Lua51 => "Lua 5.1",
        V::Lua52 => "Lua 5.2",
        V::Lua53 => "Lua 5.3",
        V::Lua54 => "Lua 5.4",
        // MacroLua reports the 5.4 base it inherits from (audit-locked).
        V::MacroLua => "Lua 5.4",
        V::Lua55 => "Lua 5.5",
    };
    g.push(("_VERSION", vm.str_value(version.as_bytes())?));
    if v == V::Lua51 {
        g.push(("ipairs", ipairs(vm)?));
        // 5.1 has no light C functions and gives pairs its own closure over
        // `luaB_next`, so `pairs{} ~= next`
        let own_next = vm.lib_native(nat_next)?;
        g.push(("pairs", vm.builtin(nat_pairs, &[own_next], Builtin::Pairs)?));
        g.push(("newproxy", newproxy(vm)?));
    }
    let globals = vm.globals();
    vm.lib_put_all(globals, "_G", g)
}

/// `ipairs`, over its iterator.
fn ipairs(vm: &mut Vm) -> Result<Value, LuaError> {
    let it = vm.builtin(ipairs_iter, &[], Builtin::IpairsIter)?;
    vm.lib_native_with(nat_ipairs, &[it])
}

/// 5.1's `newproxy`, which remembers the metatables it created in a weak
/// set (PUC's upvalue `weaktable`, its own metatable): only a userdata
/// carrying one of them counts as a proxy whose metatable may be shared.
fn newproxy(vm: &mut Vm) -> Result<Value, LuaError> {
    let proxies = vm.alloc_table()?;
    let mode_k = vm.str_value(b"__mode")?;
    let mode_v = vm.str_value(b"kv")?;
    vm.lib_set_value(proxies, mode_k, mode_v)?;
    // SAFETY: `proxies` was allocated a few lines above and is held only by this local; the borrow covers one call
    unsafe { proxies.as_mut() }.set_metatable(Some(proxies));
    vm.lib_native_with(nat_newproxy, &[Value::Table(proxies)])
}

mod basic;
mod compat51;
mod errors;
mod iter;
mod load;
mod protected;
mod tonumber;

use basic::{
    nat_getmetatable, nat_print, nat_rawequal, nat_rawget, nat_rawlen, nat_rawset, nat_select,
    nat_setmetatable, nat_tostring, nat_type, nat_warn,
};
use compat51::{nat_gcinfo, nat_getfenv, nat_newproxy, nat_setfenv};
use errors::nat_assert;
pub(crate) use errors::nat_error;
pub(crate) use errors::{
    arg_error, concat2, lual_where, raise_bytes, raise_bytes_fmt, raise_fmt, raise_lib, raise_str,
};
pub use iter::ipairs_iter;
use iter::{nat_ipairs, nat_next};
pub(crate) use iter::{nat_pairs, pairs_mm_results};
pub(crate) use load::nat_load;
use load::nat_loadstring;
pub(crate) use protected::{
    nat_host_pcall, nat_host_pcall_in_c, nat_host_xpcall, nat_host_xpcall_in_c, nat_pcall,
    nat_xpcall, xpcall_handler,
};
pub(crate) use tonumber::nat_tonumber;
