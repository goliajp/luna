//! The strings PUC makes to format a message (`luaO_pushvfstring`). Up to
//! 5.3 each piece of the format becomes a string of its own before they
//! are concatenated (`luaV_concat`), 5.1 and 5.2 in a buffer kept for all
//! concatenations; from 5.4 the pieces are put together on the C stack and
//! only the result is a string.

use super::*;
use crate::runtime::mem::Oom;
use crate::runtime::table::Dialect;

impl Heap {
    /// 5.1 and 5.2's buffer for concatenations made room for `n` bytes
    /// (`luaZ_openspace`, at least `LUA_MINBUFFER`).
    pub(crate) fn open_gbuff(&mut self, n: usize) {
        if n > self.puc.gbuff {
            let size = n.max(32);
            self.acct_block(self.puc.gbuff, size);
            self.puc.gbuff = size;
        }
    }

    /// The string of `fmt` with each `%s` replaced by the next of `args`,
    /// made as PUC's `lua_pushfstring` makes it: the pieces are strings
    /// luna makes too (left as garbage), so the count and the collector
    /// see them as PUC's.
    pub(crate) fn pushfstring_s(&mut self, fmt: &[u8], args: &[&[u8]]) -> Result<Gc<LuaStr>, Oom> {
        let d = self.table_dialect;
        if matches!(d, Dialect::L54 | Dialect::L55) {
            let mut out = Vec::new();
            let mut args = args.iter();
            for (i, seg) in fmt.split(|&c| c == b'%').enumerate() {
                if i > 0 {
                    out.extend_from_slice(args.next().copied().unwrap_or_default());
                    out.extend_from_slice(&seg[1..]);
                } else {
                    out.extend_from_slice(seg);
                }
            }
            return self.try_intern(&out);
        }
        // the stack `luaO_pushvfstring` leaves for `luaV_concat` (5.1 puts
        // an empty string first)
        let mut stack: Vec<Gc<LuaStr>> = Vec::new();
        if d == Dialect::L51 {
            stack.push(self.try_intern(b"")?);
        }
        let mut rest = fmt;
        let mut args = args.iter();
        while let Some(e) = rest.iter().position(|&c| c == b'%') {
            stack.push(self.try_intern(&rest[..e])?);
            stack.push(self.try_intern(args.next().copied().unwrap_or_default())?);
            rest = &rest[e + 2..];
        }
        stack.push(self.try_intern(rest)?);
        // 5.2 and 5.3 concatenate only when there was a conversion
        if d != Dialect::L51 && stack.len() == 1 {
            return Ok(stack[0]);
        }
        // `luaV_concat`, from the top
        while stack.len() > 1 {
            let n = stack.len();
            if stack[n - 1].is_empty() {
                stack.pop();
            } else if d != Dialect::L51 && stack[n - 2].is_empty() {
                stack.remove(n - 2);
            } else {
                let mut bytes = Vec::new();
                for s in &stack {
                    bytes.extend_from_slice(s.as_bytes());
                }
                if matches!(d, Dialect::L51 | Dialect::L52) {
                    self.open_gbuff(bytes.len());
                }
                let s = self.try_intern(&bytes)?;
                stack.clear();
                stack.push(s);
            }
        }
        Ok(stack[0])
    }
}
