//! The bit32 library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, run_open};

use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("b_and", F(b_and, Builtin::None)),
    ("b_arshift", F(b_arshift, Builtin::None)),
    ("b_extract", F(b_extract, Builtin::None)),
    ("b_lrot", F(b_lrot, Builtin::None)),
    ("b_lshift", F(b_lshift, Builtin::None)),
    ("b_not", F(b_not, Builtin::None)),
    ("b_or", F(b_or, Builtin::None)),
    ("b_replace", F(b_replace, Builtin::None)),
    ("b_rrot", F(b_rrot, Builtin::None)),
    ("b_rshift", F(b_rshift, Builtin::None)),
    ("b_test", F(b_test, Builtin::None)),
    ("b_xor", F(b_xor, Builtin::None)),
    ("luaopen_bit32", Ctor::Open(luaopen_bit32)),
];

fn luaopen_bit32(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_bit32", 1)
}
