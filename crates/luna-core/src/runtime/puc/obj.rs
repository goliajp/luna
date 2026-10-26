//! What PUC allocates at a point, named by what it is: the model works out
//! the bytes for the dialect and the target, so a library says what PUC
//! makes there and never PUC's layout.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::mem::Oom;
use records::GhostRef;

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
    /// a prototype and `n` bytes of the arrays it holds, freed with it
    ProtoWithArrays(usize),
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
            PucObj::ProtoWithArrays(n) => s.proto + n,
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

impl PucObj {
    /// The kind of object it is. A part of one (nodes, stack slots, a
    /// `CallInfo`, a string's header) is none, and is not counted as one.
    pub(in crate::runtime) fn kind(self) -> PucKind {
        match self {
            PucObj::Udata { .. }
            | PucObj::Box
            | PucObj::Stream
            | PucObj::GMatchState
            | PucObj::RandState => PucKind::Userdata,
            PucObj::CClosure(_) => PucKind::CClosure,
            PucObj::LuaClosure(_) => PucKind::LuaClosure,
            PucObj::Upval => PucKind::Upval,
            PucObj::Proto | PucObj::ProtoWithArrays(_) => PucKind::Proto,
            PucObj::Table => PucKind::Table,
            PucObj::Str { long: false, .. } => PucKind::ShortStr,
            PucObj::Str { long: true, .. } | PucObj::LongStrHeader | PucObj::ExternalStr => {
                PucKind::LongStr
            }
            PucObj::Thread => PucKind::Thread,
            PucObj::Nodes(_) | PucObj::CallInfo | PucObj::Slots(_) | PucObj::StrHeader => {
                panic!("{self:?} is a part of an object, not one")
            }
        }
    }
}

impl Heap {
    /// A new object `o`.
    #[inline(always)]
    pub(in crate::runtime) fn new_obj(&mut self, o: PucObj) {
        self.acct_obj(o.kind(), self.puc_size(o));
    }

    /// `o`, made without counting, is PUC's now: counted here.
    #[inline(always)]
    pub(in crate::runtime) fn promote_obj(&mut self, h: Gc<GcHeader>, o: PucObj) {
        self.acct_promote_obj(h, o.kind(), self.puc_size(o));
    }

    /// An object `o` PUC makes and luna does not: counted now, freed by the
    /// next sweep.
    pub(in crate::runtime) fn ghost_obj(&mut self, o: PucObj) -> Result<(), Oom> {
        self.acct_ghost(o.kind(), self.puc_size(o))
    }

    /// An object `o` PUC makes and luna does not (yet), as
    /// [`Heap::ghost_obj`]: where its record is.
    pub(in crate::runtime) fn ghost_obj_ref(&mut self, o: PucObj) -> Result<GhostRef, Oom> {
        self.acct_ghost_ref(o.kind(), self.puc_size(o))
    }

    /// `cl`, a main closure made without counting, is PUC's now: counted
    /// here, without 5.1's environment among its upvalues.
    pub(in crate::runtime) fn promote_main_closure(&mut self, cl: Gc<LuaClosure>) {
        let size = self.closure_size(&cl.proto, cl.upvals().len());
        self.acct_promote_obj(cl.hdr(), PucKind::LuaClosure, size);
    }

    /// The bytes PUC allocates for `o` in this dialect.
    #[inline]
    pub(in crate::runtime) fn puc_size(&self, o: PucObj) -> usize {
        o.bytes(self.puc.dialect, &self.puc.sizes)
    }
}
