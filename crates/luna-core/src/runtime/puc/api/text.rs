//! The strings PUC makes formatting messages and joining strings, as PUC
//! makes them.

use super::super::*;
use crate::numeric::Num;
use crate::runtime::heap::Heap;
use crate::runtime::mem::Oom;
use crate::runtime::puc::fmt::FmtArg;
use crate::runtime::puc::records::strcache::CStr;

/// The strings PUC makes formatting messages and joining strings, as PUC
/// makes them.
pub(crate) trait PucText {
    /// The text of a number as PUC writes it (`lua_number2str`,
    /// `tostringbuff`); 5.1 and 5.2 have no integers.
    fn number_text(&self, n: Num) -> String;
    /// 5.1 and 5.2's buffer for concatenations made room for `n` bytes
    /// (`luaZ_openspace`, at least `LUA_MINBUFFER`).
    fn open_gbuff(&mut self, n: usize) -> Result<(), Oom>;
    /// PUC `luaV_concat(L, total)` over the top `total` values of `st`,
    /// every one a string or a number: what it makes, in its order, and
    /// the one value it leaves in their place.
    fn puc_concat(&mut self, st: &mut Vec<Value>, total: usize) -> Result<(), Oom>;
    /// The string of `fmt` with each directive replaced by the next of
    /// `args`, made as PUC's `lua_pushfstring` makes it.
    fn pushfstring(&mut self, fmt: &[u8], args: &[FmtArg<'_>]) -> Result<Gc<LuaStr>, Oom>;
    /// The parser's string of a chunk's name (`luaY_parser`'s `luaS_new`),
    /// `name` the Lua string whose characters it is, if any: that string
    /// itself where it is interned (5.1 interns every string), else a long
    /// string through the cache.
    fn chunk_name(&mut self, chunkname: &[u8], name: Option<Gc<LuaStr>>)
    -> Result<Gc<LuaStr>, Oom>;
    /// PUC `luaS_new` of the C string at `at` holding `bytes`: from 5.3 a
    /// long string found in the cache at that address, else a new one,
    /// which the cache keeps there.
    fn intern_at(&mut self, at: CStr, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom>;
}

impl PucText for Heap {
    #[inline(always)]
    fn number_text(&self, n: Num) -> String {
        Heap::number_text(self, n)
    }

    #[inline(always)]
    fn open_gbuff(&mut self, n: usize) -> Result<(), Oom> {
        Heap::open_gbuff(self, n)
    }

    #[inline(always)]
    fn puc_concat(&mut self, st: &mut Vec<Value>, total: usize) -> Result<(), Oom> {
        Heap::puc_concat(self, st, total)
    }

    #[inline(always)]
    fn pushfstring(&mut self, fmt: &[u8], args: &[FmtArg<'_>]) -> Result<Gc<LuaStr>, Oom> {
        Heap::pushfstring(self, fmt, args)
    }

    #[inline(always)]
    fn chunk_name(
        &mut self,
        chunkname: &[u8],
        name: Option<Gc<LuaStr>>,
    ) -> Result<Gc<LuaStr>, Oom> {
        Heap::chunk_name(self, chunkname, name)
    }
    #[inline(always)]
    fn intern_at(&mut self, at: CStr, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        Heap::intern_at(self, at, bytes)
    }
}
