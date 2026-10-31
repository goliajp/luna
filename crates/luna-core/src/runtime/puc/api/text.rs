//! The strings PUC makes formatting messages and joining strings, as PUC
//! makes them.

use super::super::*;
use crate::numeric::{self, Num};
use crate::runtime::dialect::Dialect;
use crate::runtime::mem::{LVec, Oom};
use crate::runtime::puc::fmt::*;
use crate::runtime::puc::records::strcache::*;

impl Heap {
    /// The text of a number as PUC writes it (`lua_number2str`,
    /// `tostringbuff`); 5.1 and 5.2 have no integers.
    pub(crate) fn number_text(&self, n: Num) -> String {
        let n = match (self.puc.dialect, n) {
            (Dialect::L51 | Dialect::L52, Num::Int(i)) => Num::Float(i as f64),
            (_, n) => n,
        };
        numeric::num_to_string_for(n, self.float_fmt())
    }

    /// The string of `fmt` with each directive replaced by the next of
    /// `args`, made as PUC's `lua_pushfstring` makes it.
    pub(crate) fn pushfstring(
        &mut self,
        fmt: &[u8],
        args: &[FmtArg<'_>],
    ) -> Result<Gc<LuaStr>, Oom> {
        match self.puc.dialect {
            Dialect::L54 => self.pushfstring_54(fmt, args),
            Dialect::L55 => self.pushfstring_55(fmt, args),
            _ => self.pushfstring_pieces(fmt, args),
        }
    }

    /// 5.1 and 5.2's buffer for concatenations made room for `n` bytes
    /// (`luaZ_openspace`, at least `LUA_MINBUFFER`).
    pub(crate) fn open_gbuff(&mut self, n: usize) -> Result<(), Oom> {
        let old = self.puc.records.gbuff.size();
        if n > old {
            let mem = self.mem();
            let size = n.max(32);
            let b = self
                .puc
                .records
                .gbuff
                .0
                .get_or_insert_with(|| LVec::new(mem));
            b.reserve_exact(size)?;
            let waited = self.refusal_waits();
            self.acct_part(old, size);
            // gc-trace: `LUNA_GC_REFUSE` refused this growth of PUC's, which is
            // then as if the allocation failed
            if let Some(bytes) = self.take_new_refusal(waited) {
                self.adjust_count(-bytes);
                self.shrink_gbuff_to(old);
                return Err(Oom(mem));
            }
        }
        Ok(())
    }

    /// PUC `luaV_concat(L, *top)` over the first `*top` values of `st`,
    /// every one a string or a number: what it makes, in its order, joined
    /// from the last as on PUC's stack. `*top` comes down as values are
    /// joined: 1 when it succeeds, the value in `st[0]`; where it stopped
    /// when it fails.
    pub(crate) fn puc_concat(&mut self, st: &mut [Value], top: &mut usize) -> Result<(), Oom> {
        let d = self.puc.dialect;
        while *top > 1 {
            let at = *top;
            self.tostring_value(&mut st[at - 1])?;
            let Value::Str(y) = st[at - 1] else {
                unreachable!("converted above")
            };
            let mut n = 2;
            if y.is_empty() {
                self.tostring_value(&mut st[at - 2])?;
            } else if d >= Dialect::L52 && matches!(st[at - 2], Value::Str(s) if s.is_empty()) {
                st[at - 2] = Value::Str(y);
            } else {
                let mut tl = y.len();
                n = 1;
                while n < at {
                    self.tostring_value(&mut st[at - n - 1])?;
                    let Value::Str(s) = st[at - n - 1] else {
                        unreachable!("converted above")
                    };
                    tl += s.len();
                    n += 1;
                }
                if d <= Dialect::L52 {
                    self.open_gbuff(tl)?;
                }
                let parts: Vec<&[u8]> = st[at - n..at]
                    .iter()
                    .map(|v| match v {
                        Value::Str(s) => s.as_bytes(),
                        _ => unreachable!("converted above"),
                    })
                    .collect();
                let s = self.try_intern_parts(&parts)?;
                st[at - n] = Value::Str(s);
            }
            *top = at - (n - 1);
        }
        Ok(())
    }

    /// PUC `luaS_new` of the C string at `at` holding `bytes`: from 5.3 a
    /// long string found in the cache at that address, else a new one,
    /// which the cache keeps there.
    pub(crate) fn intern_at(&mut self, at: CStr, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        let cached = matches!(self.puc.dialect, Dialect::L53 | Dialect::L54 | Dialect::L55)
            && bytes.len() > string::MAX_SHORT_LEN;
        if let Some(s) = self.puc.records.strcache.find(at, bytes).filter(|_| cached) {
            return Ok(s);
        }
        let s = self.try_intern(bytes)?;
        if cached {
            self.puc.records.strcache.put(at, s);
        }
        Ok(s)
    }

    /// The parser's string of a chunk's name (`luaY_parser`'s `luaS_new`),
    /// `name` the Lua string whose characters it is, if any: that string
    /// itself where it is interned (5.1 interns every string), else a long
    /// string through the cache.
    pub(crate) fn chunk_name(
        &mut self,
        chunkname: &[u8],
        name: Option<Gc<LuaStr>>,
    ) -> Result<Gc<LuaStr>, Oom> {
        let long = chunkname.len() > string::MAX_SHORT_LEN && self.puc.dialect != Dialect::L51;
        match name {
            Some(n) if !long => Ok(n),
            Some(n) => self.intern_at(CStr::In(n.as_ptr() as *const GcHeader, 0), chunkname),
            None => self.try_intern(chunkname),
        }
    }

    /// Reserved word number `k` (in the order of 5.5's `luaX_tokens`),
    /// which the state made and fixed, if the dialect has it.
    #[inline]
    pub(crate) fn reserved_word(&self, k: u8) -> Option<Gc<LuaStr>> {
        self.puc.records.reserved[usize::from(k)]
    }
}
