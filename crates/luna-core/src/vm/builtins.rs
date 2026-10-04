//! Minimal base library. The full base library replaces/extends this.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_base(vm: &mut Vm) {
    let f = vm.native(nat_assert);
    vm.set_global("assert", f).expect("stdlib registration");
    let f = vm.native(nat_error);
    vm.set_global("error", f).expect("stdlib registration");
    let f = vm.native(nat_pcall);
    vm.set_global("pcall", f).expect("stdlib registration");
    let f = vm.native(nat_xpcall);
    vm.set_global("xpcall", f).expect("stdlib registration");
    let f = vm.native(nat_type);
    vm.set_global("type", f).expect("stdlib registration");
    let f = vm.native(nat_print);
    vm.set_global("print", f).expect("stdlib registration");
    let f = vm.native(nat_tostring);
    vm.set_global("tostring", f).expect("stdlib registration");
    let f = vm.native(nat_rawget);
    vm.set_global("rawget", f).expect("stdlib registration");
    let f = vm.native(nat_rawset);
    vm.set_global("rawset", f).expect("stdlib registration");
    let f = vm.native(nat_rawequal);
    vm.set_global("rawequal", f).expect("stdlib registration");
    // `rawlen` arrived in 5.2.
    if vm.version() >= crate::version::LuaVersion::Lua52 {
        let f = vm.native(nat_rawlen);
        vm.set_global("rawlen", f).expect("stdlib registration");
    }
    let f = vm.native(nat_setmetatable);
    vm.set_global("setmetatable", f)
        .expect("stdlib registration");
    let f = vm.native(nat_getmetatable);
    vm.set_global("getmetatable", f)
        .expect("stdlib registration");
    let f = vm.native(nat_select);
    vm.set_global("select", f).expect("stdlib registration");
    let next_obj = vm.native(nat_next);
    vm.set_global("next", next_obj)
        .expect("stdlib registration");
    // 5.2+ pairs returns the global next itself (a light C function, equal
    // to every other push of `luaB_next`); 5.1 has no light C functions and
    // gives pairs its own closure over `luaB_next`, so `pairs{} ~= next`.
    let pairs_next = if vm.version() == LuaVersion::Lua51 {
        vm.native(nat_next)
    } else {
        next_obj
    };
    let pairs_obj = vm.native_with(nat_pairs, Box::new([pairs_next]));
    vm.set_global("pairs", pairs_obj)
        .expect("stdlib registration");
    let ipairs_it = vm.native(ipairs_iter);
    let ipairs_obj = vm.native_with(nat_ipairs, Box::new([ipairs_it]));
    vm.set_global("ipairs", ipairs_obj)
        .expect("stdlib registration");
    let f = vm.native(nat_tonumber);
    vm.set_global("tonumber", f).expect("stdlib registration");
    let load_obj = vm.native(nat_load);
    vm.set_global("load", load_obj)
        .expect("stdlib registration");
    let f = vm.native(crate::vm::lib_gc::nat_collectgarbage);
    vm.set_global("collectgarbage", f)
        .expect("stdlib registration");
    // PUC 5.4 introduced the warning system. `warn(msg1, …, msgN)` emits
    // pieces of one message via the default warnf (`lauxlib.c::warnfon/off`),
    // which recognises `@on` / `@off` control messages and starts disabled.
    if vm.version() >= crate::version::LuaVersion::Lua54 {
        let f = vm.native(nat_warn);
        vm.set_global("warn", f).expect("stdlib registration");
    }
    // PUC 5.2's official build ships -DLUA_COMPAT_ALL, so `loadstring`
    // survives as a `load` alias there too — the diff ground truth is
    // the default build (v2.14 dialect fixture 5.2/521).
    if vm.version() == crate::version::LuaVersion::Lua52 {
        vm.set_global("loadstring", load_obj)
            .expect("stdlib registration");
    }
    // PUC 5.1 globals retired in 5.2 (`unpack` → `table.unpack`) and 5.2
    // (`loadstring` → `load`). Provide aliases so the 5.1 test suite, which
    // is full of `unpack(...)` and `loadstring("...")` calls, still resolves.
    if vm.version() == crate::version::LuaVersion::Lua51 {
        let f = vm.native(nat_loadstring);
        vm.set_global("loadstring", f).expect("stdlib registration");
        let f = vm.native(crate::vm::lib_table::t_unpack);
        vm.set_global("unpack", f).expect("stdlib registration");
        // PUC 5.1 also exposed `gcinfo()` (memory in KB) and `newproxy()`.
        let f = vm.native(nat_gcinfo);
        vm.set_global("gcinfo", f).expect("stdlib registration");
        // PUC 5.1 `setfenv`/`getfenv` — every Lua function carries its own
        // env (5.1 `LClosure.env`); 5.2 retired them in favour of the `_ENV`
        // upvalue model. A 5.1 closure shares its creator's `_ENV` cell and
        // `setfenv` replaces the cell, so the change only affects that
        // closure (events.lua / locals.lua / nextvar.lua).
        let f = vm.native(nat_setfenv);
        vm.set_global("setfenv", f).expect("stdlib registration");
        let f = vm.native(nat_getfenv);
        vm.set_global("getfenv", f).expect("stdlib registration");
        // `newproxy` remembers the metatables it created in a weak-keyed
        // set (PUC's upvalue `weaktable`): only a userdata carrying one of
        // them counts as a proxy whose metatable may be shared.
        let proxies = vm.heap.new_table();
        let weak_k = vm.heap.new_table();
        let mode_k = Value::Str(vm.heap.intern(b"__mode"));
        let mode_v = Value::Str(vm.heap.intern(b"k"));
        // SAFETY: `weak_k` is the table allocated above, so it is alive; no reference into it is held across this call, and `set` does not collect
        unsafe { weak_k.as_mut() }
            .set(&mut vm.heap, mode_k, mode_v)
            .expect("valid key");
        // SAFETY: `proxies` was allocated a few lines above and is held only by this local; the borrow covers one call
        unsafe { proxies.as_mut() }.set_metatable(Some(weak_k));
        let f = vm.native_with(nat_newproxy, Box::new([Value::Table(proxies)]));
        vm.set_global("newproxy", f).expect("stdlib registration");
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
    let v = Value::Str(vm.heap.intern(version.as_bytes()));
    vm.set_global("_VERSION", v).expect("stdlib registration");
    let g = Value::Table(vm.globals());
    vm.set_global("_G", g).expect("stdlib registration");
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
pub(crate) use errors::{arg_error, raise_bytes, raise_str};
use errors::{nat_assert, nat_error};
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
