//! The utf8 library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, run_open};

use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("byteoffset", F(u_offset, Builtin::None)),
    ("codepoint", F(u_codepoint, Builtin::None)),
    ("iter_codes", F(u_codes, Builtin::None)),
    ("luaopen_utf8", Ctor::Open(luaopen_utf8)),
    ("utfchar", Ctor::R(by_rules!(u_char), Builtin::None)),
    ("utflen", F(u_len, Builtin::None)),
];

fn luaopen_utf8(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_utf8", 1)
}
