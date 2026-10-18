//! The base library's C functions as PUC's opening names them (with 5.1's
//! coroutine functions, which its base library registers too).

use super::*;
use crate::runtime::{Builtin, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::opening::{Ctor, Natives, Opener, run_open};

use Ctor::{C, F};

pub(crate) const PUC_FNS: Natives = &[
    ("ipairsaux", C(ipairsaux)),
    ("luaB_assert", F(nat_assert, Builtin::None)),
    (
        "luaB_collectgarbage",
        F(crate::vm::lib_gc::nat_collectgarbage, Builtin::None),
    ),
    (
        "luaB_dofile",
        F(crate::vm::lib_os_io::nat_dofile, Builtin::None),
    ),
    ("luaB_error", F(nat_error, Builtin::Error)),
    ("luaB_gcinfo", F(nat_gcinfo, Builtin::None)),
    ("luaB_getfenv", F(nat_getfenv, Builtin::None)),
    ("luaB_getmetatable", F(nat_getmetatable, Builtin::None)),
    ("luaB_ipairs", C(luab_ipairs)),
    ("luaB_load", F(nat_load, Builtin::None)),
    (
        "luaB_loadfile",
        F(crate::vm::lib_os_io::nat_loadfile, Builtin::None),
    ),
    ("luaB_loadstring", F(nat_loadstring, Builtin::None)),
    ("luaB_newproxy", F(nat_newproxy, Builtin::None)),
    ("luaB_next", F(nat_next, Builtin::None)),
    ("luaB_pairs", C(luab_pairs)),
    ("luaB_pcall", F(nat_pcall, Builtin::Pcall)),
    ("luaB_print", F(nat_print, Builtin::None)),
    ("luaB_rawequal", F(nat_rawequal, Builtin::None)),
    ("luaB_rawget", F(nat_rawget, Builtin::None)),
    ("luaB_rawlen", F(nat_rawlen, Builtin::None)),
    ("luaB_rawset", F(nat_rawset, Builtin::None)),
    ("luaB_select", F(nat_select, Builtin::None)),
    ("luaB_setfenv", F(nat_setfenv, Builtin::None)),
    ("luaB_setmetatable", F(nat_setmetatable, Builtin::None)),
    ("luaB_tonumber", F(nat_tonumber, Builtin::None)),
    ("luaB_tostring", F(nat_tostring, Builtin::Tostring)),
    ("luaB_type", F(nat_type, Builtin::None)),
    (
        "luaB_unpack",
        F(crate::vm::lib_table::t_unpack, Builtin::None),
    ),
    ("luaB_warn", F(nat_warn, Builtin::None)),
    ("luaB_xpcall", F(nat_xpcall, Builtin::Xpcall)),
    ("luaopen_base", Ctor::Open(luaopen_base)),
];

fn luaopen_base(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.1's returns the base library and the coroutine library
    let n = if vm.version() == crate::version::LuaVersion::Lua51 {
        2
    } else {
        1
    };
    run_open(vm, fs, nargs, "luaopen_base", n)
}

/// 5.1's iterator of `ipairs`, its upvalue.
fn ipairsaux(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    op.func(vm, id, ipairs_iter, Builtin::IpairsIter, &[], nup)
}

/// `ipairs`: luna's keeps its iterator; 5.1's is PUC's upvalue, later
/// dialects' a light function luna gives one of its own.
fn luab_ipairs(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let it = match nup {
        0 => vm.builtin(ipairs_iter, &[], Builtin::IpairsIter)?,
        _ => op.upval(vm, 0, nup),
    };
    op.func(vm, id, nat_ipairs, Builtin::None, &[it], nup)
}

/// `pairs`: luna's keeps the `next` it returns; 5.1's is PUC's upvalue,
/// later dialects' the global `next`, set before it.
fn luab_pairs(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let next = match nup {
        0 => op.global(vm, b"next"),
        _ => op.upval(vm, 0, nup),
    };
    op.func(vm, id, nat_pairs, Builtin::Pairs, &[next], nup)
}
