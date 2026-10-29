//! The os library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, run_open};

use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("luaopen_os", Ctor::Open(luaopen_os)),
    ("os_clock", F(os_clock, Builtin::None)),
    ("os_date", Ctor::R(by_rules!(os_date), Builtin::None)),
    ("os_difftime", F(os_difftime, Builtin::None)),
    ("os_execute", F(os_execute, Builtin::None)),
    ("os_exit", F(os_exit, Builtin::None)),
    ("os_getenv", F(os_getenv, Builtin::None)),
    ("os_remove", F(os_remove, Builtin::None)),
    ("os_rename", F(os_rename, Builtin::None)),
    ("os_setlocale", F(os_setlocale, Builtin::None)),
    ("os_time", F(os_time, Builtin::None)),
    ("os_tmpname", F(os_tmpname, Builtin::None)),
];

fn luaopen_os(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_os", 1)
}
