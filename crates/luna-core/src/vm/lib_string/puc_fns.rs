//! The string library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, run_open};

use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("arith_add", F(mm_add, Builtin::None)),
    ("arith_div", F(mm_div, Builtin::None)),
    ("arith_idiv", F(mm_idiv, Builtin::None)),
    ("arith_mod", F(mm_mod, Builtin::None)),
    ("arith_mul", F(mm_mul, Builtin::None)),
    ("arith_pow", F(mm_pow, Builtin::None)),
    ("arith_sub", F(mm_sub, Builtin::None)),
    ("arith_unm", F(mm_unm, Builtin::None)),
    ("gfind_nodef", F(gfind_nodef, Builtin::None)),
    ("gmatch", F(s_gmatch, Builtin::None)),
    ("luaopen_string", Ctor::Open(luaopen_string)),
    ("str_byte", F(s_byte, Builtin::None)),
    ("str_char", F(s_char, Builtin::None)),
    ("str_dump", F(s_dump, Builtin::None)),
    ("str_find", F(s_find, Builtin::None)),
    (
        "str_format",
        F(crate::vm::lib_strformat::s_format, Builtin::None),
    ),
    ("str_gsub", F(s_gsub, Builtin::None)),
    ("str_len", F(s_len, Builtin::None)),
    ("str_lower", F(s_lower, Builtin::None)),
    ("str_match", F(s_match, Builtin::None)),
    ("str_pack", F(crate::vm::lib_strpack::s_pack, Builtin::None)),
    (
        "str_packsize",
        F(crate::vm::lib_strpack::s_packsize, Builtin::None),
    ),
    ("str_rep", F(s_rep, Builtin::None)),
    ("str_reverse", F(s_reverse, Builtin::None)),
    ("str_sub", F(s_sub, Builtin::StringSub)),
    (
        "str_unpack",
        F(crate::vm::lib_strpack::s_unpack, Builtin::None),
    ),
    ("str_upper", F(s_upper, Builtin::None)),
];

fn luaopen_string(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_string", 1)
}

/// 5.1's `gfind` without its compatibility: replaced by `gmatch` as the
/// library opens.
fn gfind_nodef(vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Err(vm.rt_err("'string.gfind' was renamed to 'string.gmatch'"))
}
