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
    // the globals, put in the order PUC's base library puts them
    let mut g: Vec<(&str, Value)> = Vec::with_capacity(32);
    let f = vm.lib_native(nat_assert)?;
    g.push(("assert", f));
    let f = vm.builtin(nat_error, &[], Builtin::Error)?;
    g.push(("error", f));
    let f = vm.builtin(nat_pcall, &[], Builtin::Pcall)?;
    g.push(("pcall", f));
    let f = vm.builtin(nat_xpcall, &[], Builtin::Xpcall)?;
    g.push(("xpcall", f));
    let f = vm.lib_native(nat_type)?;
    g.push(("type", f));
    let f = vm.lib_native(nat_print)?;
    g.push(("print", f));
    let f = vm.builtin(nat_tostring, &[], Builtin::Tostring)?;
    g.push(("tostring", f));
    let f = vm.lib_native(nat_rawget)?;
    g.push(("rawget", f));
    let f = vm.lib_native(nat_rawset)?;
    g.push(("rawset", f));
    let f = vm.lib_native(nat_rawequal)?;
    g.push(("rawequal", f));
    // `rawlen` arrived in 5.2.
    if vm.version() >= crate::version::LuaVersion::Lua52 {
        let f = vm.lib_native(nat_rawlen)?;
        g.push(("rawlen", f));
    }
    let f = vm.lib_native(nat_setmetatable)?;
    g.push(("setmetatable", f));
    let f = vm.lib_native(nat_getmetatable)?;
    g.push(("getmetatable", f));
    let f = vm.lib_native(nat_select)?;
    g.push(("select", f));
    let next_obj = vm.lib_native(nat_next)?;
    g.push(("next", next_obj));
    // 5.2+ pairs returns the global next itself (a light C function, equal
    // to every other push of `luaB_next`); 5.1 has no light C functions and
    // gives pairs its own closure over `luaB_next`, so `pairs{} ~= next`.
    let pairs_next = if vm.version() == LuaVersion::Lua51 {
        vm.lib_native(nat_next)?
    } else {
        next_obj
    };
    let pairs_obj = vm.builtin(nat_pairs, &[pairs_next], Builtin::Pairs)?;
    g.push(("pairs", pairs_obj));
    let ipairs_it = vm.builtin(ipairs_iter, &[], Builtin::IpairsIter)?;
    let ipairs_obj = vm.lib_native_with(nat_ipairs, &[ipairs_it])?;
    g.push(("ipairs", ipairs_obj));
    let f = vm.lib_native(nat_tonumber)?;
    g.push(("tonumber", f));
    let load_obj = vm.lib_native(nat_load)?;
    g.push(("load", load_obj));
    let f = vm.lib_native(crate::vm::lib_gc::nat_collectgarbage)?;
    g.push(("collectgarbage", f));
    // PUC 5.4 introduced the warning system. `warn(msg1, …, msgN)` emits
    // pieces of one message via the default warnf (`lauxlib.c::warnfon/off`),
    // which recognises `@on` / `@off` control messages and starts disabled.
    if vm.version() >= crate::version::LuaVersion::Lua54 {
        let f = vm.lib_native(nat_warn)?;
        g.push(("warn", f));
    }
    // PUC 5.2's official build ships -DLUA_COMPAT_ALL, so `loadstring`
    // survives as a `load` alias there too — the diff ground truth is
    // the default build (v2.14 dialect fixture 5.2/521).
    if vm.version() == crate::version::LuaVersion::Lua52 {
        g.push(("loadstring", load_obj));
    }
    // PUC 5.1 globals retired in 5.2 (`unpack` → `table.unpack`) and 5.2
    // (`loadstring` → `load`). Provide aliases so the 5.1 test suite, which
    // is full of `unpack(...)` and `loadstring("...")` calls, still resolves.
    if vm.version() == crate::version::LuaVersion::Lua51 {
        let f = vm.lib_native(nat_loadstring)?;
        g.push(("loadstring", f));
        let f = vm.lib_native(crate::vm::lib_table::t_unpack)?;
        g.push(("unpack", f));
        // PUC 5.1 also exposed `gcinfo()` (memory in KB) and `newproxy()`.
        let f = vm.lib_native(nat_gcinfo)?;
        g.push(("gcinfo", f));
        // PUC 5.1 `setfenv`/`getfenv` — every Lua function carries its own
        // env (5.1 `LClosure.env`); 5.2 retired them in favour of the `_ENV`
        // upvalue model. A 5.1 closure shares its creator's `_ENV` cell and
        // `setfenv` replaces the cell, so the change only affects that
        // closure (events.lua / locals.lua / nextvar.lua).
        let f = vm.lib_native(nat_setfenv)?;
        g.push(("setfenv", f));
        let f = vm.lib_native(nat_getfenv)?;
        g.push(("getfenv", f));
        // `newproxy` remembers the metatables it created in a weak set
        // (PUC's upvalue `weaktable`, its own metatable): only a userdata
        // carrying one of them counts as a proxy whose metatable may be
        // shared
        let proxies = vm.alloc_table()?;
        let mode_k = vm.str_value(b"__mode")?;
        let mode_v = vm.str_value(b"kv")?;
        vm.lib_set_value(proxies, mode_k, mode_v)?;
        // SAFETY: `proxies` was allocated a few lines above and is held only by this local; the borrow covers one call
        unsafe { proxies.as_mut() }.set_metatable(Some(proxies));
        let f = vm.lib_native_with(nat_newproxy, &[Value::Table(proxies)])?;
        g.push(("newproxy", f));
    }
    let version = match vm.version() {
        crate::version::LuaVersion::Lua51 => "Lua 5.1",
        crate::version::LuaVersion::Lua52 => "Lua 5.2",
        crate::version::LuaVersion::Lua53 => "Lua 5.3",
        crate::version::LuaVersion::Lua54 => "Lua 5.4",
        // MacroLua reports the 5.4 base it inherits from (audit-locked).
        crate::version::LuaVersion::MacroLua => "Lua 5.4",
        crate::version::LuaVersion::Lua55 => "Lua 5.5",
    };
    let v = vm.str_value(version.as_bytes())?;
    g.push(("_VERSION", v));
    g.push(("_G", Value::Table(vm.globals())));
    if loaders {
        g.push((
            "loadfile",
            vm.lib_native(crate::vm::lib_os_io::nat_loadfile)?,
        ));
        g.push(("dofile", vm.lib_native(crate::vm::lib_os_io::nat_dofile)?));
    }
    let globals = vm.globals();
    vm.lib_put_all(globals, "_G", g)
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
