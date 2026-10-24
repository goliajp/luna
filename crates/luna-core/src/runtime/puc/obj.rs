//! What PUC allocates at a point, named by what it is: the model works out
//! the bytes for the dialect and the target, so a library says what PUC
//! makes there and never PUC's layout.

use super::*;
use crate::runtime::dialect::Dialect;

/// One allocation of PUC's, by what it is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PucObj {
    /// a full userdata of `bytes` with `nuv` user values (up to 5.3 one,
    /// whatever `nuv` says)
    Udata {
        nuv: usize,
        bytes: usize,
    },
    /// `luaL_Buffer`'s box (`newbox`): two pointers, without a user value
    /// from 5.4 on
    Box,
    /// the io library's file handle: 5.1 a `FILE *`, later a `luaL_Stream`
    /// (the file and its close function), from 5.4 without a user value
    Stream,
    /// 5.3 on, `gmatch`'s `GMatchState`
    GMatchState,
    /// math's random state (`luaL_newudata` of 32 bytes)
    RandState,
    CClosure(usize),
    LuaClosure(usize),
    Upval,
    Proto,
    Table,
    /// a table's hash part of `n` nodes
    Nodes(usize),
    CallInfo,
    /// `n` stack slots
    Slots(usize),
    /// a string of `len` bytes, long or short
    Str {
        len: usize,
        long: bool,
    },
    /// a long string's header alone (a 5.5 external string)
    LongStrHeader,
    /// 5.5 `lua_pushexternalstring`'s string: a long string's header with
    /// its free function and that function's argument, the bytes outside
    ExternalStr,
    /// a string's header alone, what 5.2 and 5.3 count as the work of
    /// marking a string or a userdata
    StrHeader,
    /// a thread without its stack and CallInfos
    Thread,
}

impl PucObj {
    /// A full userdata of `bytes` with `nuv` user values.
    pub(crate) const fn udata(nuv: usize, bytes: usize) -> PucObj {
        PucObj::Udata { nuv, bytes }
    }

    /// Its bytes in dialect `d` with sizes `s`.
    pub(crate) fn bytes(self, d: Dialect, s: &size::PucSizes) -> usize {
        match self {
            PucObj::Udata { nuv, bytes } => s.userdata(nuv, bytes),
            PucObj::Box => {
                let ubox = 2 * std::mem::size_of::<usize>();
                s.userdata(usize::from(d < Dialect::L54), ubox)
            }
            PucObj::Stream => match d {
                Dialect::L51 => s.userdata(1, s.ptr),
                Dialect::L52 | Dialect::L53 => s.userdata(1, 2 * s.ptr),
                _ => s.userdata(0, 2 * s.ptr),
            },
            PucObj::GMatchState => s.userdata(0, 71 * s.ptr + 8),
            PucObj::RandState => s.userdata(0, 32),
            PucObj::CClosure(n) => s.c_closure(n),
            PucObj::LuaClosure(n) => s.lua_closure(n),
            PucObj::Upval => s.upval,
            PucObj::Proto => s.proto,
            PucObj::Table => s.table,
            PucObj::Nodes(n) => s.node * n,
            PucObj::CallInfo => s.callinfo,
            PucObj::Slots(n) => s.tvalue * n,
            PucObj::Str { len, long } => s.string(len, long && d == Dialect::L55),
            PucObj::LongStrHeader => s.lngstr_hdr,
            PucObj::ExternalStr => s.lngstr_hdr + 2 * s.ptr,
            PucObj::StrHeader => s.str_hdr,
            PucObj::Thread => s.thread,
        }
    }
}

impl Heap {
    /// The bytes PUC allocates for `o` in this dialect.
    #[inline]
    pub(crate) fn puc_size(&self, o: PucObj) -> usize {
        o.bytes(self.puc.dialect, &self.puc.sizes)
    }
}
