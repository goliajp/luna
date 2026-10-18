//! The debug library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, run_open};

use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("db_debug", F(d_debug, Builtin::None)),
    ("db_errorfb", F(d_traceback, Builtin::None)),
    ("db_getfenv", F(d_getfenv, Builtin::None)),
    ("db_gethook", F(d_gethook, Builtin::None)),
    ("db_getinfo", F(d_getinfo, Builtin::None)),
    ("db_getlocal", F(d_getlocal, Builtin::None)),
    ("db_getmetatable", F(d_getmetatable, Builtin::None)),
    ("db_getregistry", F(d_getregistry, Builtin::None)),
    ("db_getupvalue", F(d_getupvalue, Builtin::None)),
    ("db_getuservalue", F(d_getuservalue, Builtin::None)),
    ("db_setcstacklimit", F(d_setcstacklimit, Builtin::None)),
    ("db_setfenv", F(d_setfenv, Builtin::None)),
    ("db_sethook", F(d_sethook, Builtin::None)),
    ("db_setlocal", F(d_setlocal, Builtin::None)),
    ("db_setmetatable", F(d_setmetatable, Builtin::None)),
    ("db_setupvalue", F(d_setupvalue, Builtin::None)),
    ("db_setuservalue", F(d_setuservalue, Builtin::None)),
    ("db_traceback", F(d_traceback, Builtin::None)),
    ("db_upvalueid", F(d_upvalueid, Builtin::None)),
    ("db_upvaluejoin", F(d_upvaluejoin, Builtin::None)),
    ("luaopen_debug", Ctor::Open(luaopen_debug)),
];

fn luaopen_debug(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_debug", 1)
}
