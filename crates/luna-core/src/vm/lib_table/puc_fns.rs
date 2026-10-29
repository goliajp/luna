//! The table library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, run_open};

use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("foreach", F(t_foreach, Builtin::None)),
    ("foreachi", F(t_foreachi, Builtin::None)),
    ("getn", F(t_getn, Builtin::None)),
    ("luaopen_table", Ctor::Open(luaopen_table)),
    ("maxn", F(t_maxn, Builtin::None)),
    ("pack", Ctor::R(by_rules!(t_pack), Builtin::None)),
    ("setn", F(t_setn, Builtin::None)),
    ("sort", F(t_sort, Builtin::None)),
    ("tconcat", Ctor::R(by_rules!(t_concat), Builtin::None)),
    ("tcreate", Ctor::R(by_rules!(t_create), Builtin::None)),
    ("tinsert", F(t_insert, Builtin::None)),
    ("tmove", F(t_move, Builtin::None)),
    ("tpack", Ctor::R(by_rules!(t_pack), Builtin::None)),
    ("tremove", F(t_remove, Builtin::None)),
    ("tunpack", F(t_unpack, Builtin::None)),
    ("unpack", F(t_unpack, Builtin::None)),
];

fn luaopen_table(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_table", 1)
}
