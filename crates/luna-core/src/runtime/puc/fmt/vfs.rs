//! 5.4's and 5.5's `luaO_pushvfstring`: the pieces gathered in a buffer
//! (`BuffFS`) instead of one string each.

use super::*;

/// 5.4's `BUFVFS`.
const BUFVFS_54: usize = 60 + 44 + 95;
/// 5.5's `BUFVFS`.
const BUFVFS_55: usize = 60 + 64 + 95;
/// 5.4's `MAXNUMBER2STR`: the room a numeral asks of the buffer.
const MAXNUMBER2STR_54: usize = 44;

impl Heap {
    /// 5.4: the pieces gathered in a buffer of `BUFVFS` bytes; each time it
    /// fills (and at the end) its contents become a string, joined to the
    /// one before.
    pub(super) fn pushfstring_54(
        &mut self,
        fmt: &[u8],
        args: &[FmtArg<'_>],
    ) -> Result<Gc<LuaStr>, Oom> {
        let mut b = Bufvfs {
            space: Vec::with_capacity(BUFVFS_54),
            pushed: None,
        };
        let mut rest = fmt;
        let mut args = args.iter();
        while let Some(e) = rest.iter().position(|&c| c == b'%') {
            b.add_str(self, &rest[..e])?;
            let spec = rest.get(e + 1).copied().unwrap_or(0);
            match (spec, if spec == b'%' { None } else { args.next() }) {
                (b'%', _) => b.add_str(self, b"%")?,
                (_, Some(FmtArg::S(s))) => b.add_str(self, s)?,
                (_, Some(&FmtArg::C(c))) => b.add_str(self, &[c])?,
                (_, Some(&FmtArg::D(i) | &FmtArg::I(i))) => {
                    b.get(self, MAXNUMBER2STR_54)?;
                    let t = self.number_text(Num::Int(i));
                    b.space.extend_from_slice(t.as_bytes());
                }
                (_, Some(&FmtArg::F(f))) => {
                    b.get(self, MAXNUMBER2STR_54)?;
                    let t = self.number_text(Num::Float(f));
                    b.space.extend_from_slice(t.as_bytes());
                }
                (_, Some(&FmtArg::P(p))) => {
                    b.get(self, 3 * std::mem::size_of::<usize>() + 8)?;
                    let t = crate::numeric::c_pointer(p);
                    b.space.extend_from_slice(t.as_bytes());
                }
                (_, Some(&FmtArg::U(u))) => {
                    let mut t = [0u8; 8];
                    let l = utf8_esc(&mut t, u);
                    b.add_str(self, &t[..l])?;
                }
                (_, None) => b.add_str(self, &[b'%', spec])?,
            }
            rest = &rest[(e + 2).min(rest.len())..];
        }
        b.add_str(self, rest)?;
        b.clear(self)?;
        Ok(b.pushed.expect("cleared at least once"))
    }

    /// 5.5: one buffer, on the C stack until a piece does not fit, then a
    /// block grown by each such piece (counted); one string at the end.
    pub(super) fn pushfstring_55(
        &mut self,
        fmt: &[u8],
        args: &[FmtArg<'_>],
    ) -> Result<Gc<LuaStr>, Oom> {
        let mut out: Vec<u8> = Vec::new();
        let mut size = BUFVFS_55;
        let mut heap_block = false;
        let mut add = |h: &mut Heap, out: &mut Vec<u8>, s: &[u8]| {
            if s.len() > size - out.len() {
                let new = size + s.len();
                if heap_block {
                    h.acct_block(size, new);
                } else {
                    h.acct_block(0, new);
                    heap_block = true;
                }
                size = new;
            }
            out.extend_from_slice(s);
        };
        let mut rest = fmt;
        let mut args = args.iter();
        while let Some(e) = rest.iter().position(|&c| c == b'%') {
            add(self, &mut out, &rest[..e]);
            let spec = rest.get(e + 1).copied().unwrap_or(0);
            match (spec, if spec == b'%' { None } else { args.next() }) {
                (b'%', _) => add(self, &mut out, b"%"),
                (_, Some(FmtArg::S(s))) => add(self, &mut out, s),
                (_, Some(&FmtArg::C(c))) => add(self, &mut out, &[c]),
                (_, Some(&FmtArg::D(i) | &FmtArg::I(i))) => {
                    let t = self.number_text(Num::Int(i));
                    add(self, &mut out, t.as_bytes());
                }
                (_, Some(&FmtArg::F(f))) => {
                    let t = self.number_text(Num::Float(f));
                    add(self, &mut out, t.as_bytes());
                }
                (_, Some(&FmtArg::P(p))) => {
                    let t = crate::numeric::c_pointer(p);
                    add(self, &mut out, t.as_bytes());
                }
                (_, Some(&FmtArg::U(u))) => {
                    let mut t = [0u8; 8];
                    let l = utf8_esc(&mut t, u);
                    add(self, &mut out, &t[..l]);
                }
                (_, None) => add(self, &mut out, &rest[e..(e + 2).min(rest.len())]),
            }
            rest = &rest[(e + 2).min(rest.len())..];
        }
        add(self, &mut out, rest);
        let s = self.try_intern(&out)?;
        if heap_block {
            self.acct_block(size, 0);
        }
        Ok(s)
    }
}

/// 5.4's `BuffFS`: the bytes not yet made a string, and the string of
/// what was before them.
struct Bufvfs {
    space: Vec<u8>,
    pushed: Option<Gc<LuaStr>>,
}

impl Bufvfs {
    /// `pushstr`: a string of `s`, joined to the one pushed before.
    fn push(&mut self, h: &mut Heap, s: &[u8]) -> Result<(), Oom> {
        let new = h.try_intern(s)?;
        self.pushed = Some(match self.pushed {
            None => new,
            Some(prev) => {
                let mut st = vec![Value::Str(prev), Value::Str(new)];
                h.puc_concat(&mut st, 2)?;
                match st[0] {
                    Value::Str(s) => s,
                    _ => unreachable!("strings join into a string"),
                }
            }
        });
        Ok(())
    }

    /// `clearbuff`
    fn clear(&mut self, h: &mut Heap) -> Result<(), Oom> {
        let space = std::mem::take(&mut self.space);
        self.push(h, &space)?;
        self.space = space;
        self.space.clear();
        Ok(())
    }

    /// `getbuff`: room for `sz` bytes, the buffer emptied when it has less
    fn get(&mut self, h: &mut Heap, sz: usize) -> Result<(), Oom> {
        if sz > BUFVFS_54 - self.space.len() {
            self.clear(h)?;
        }
        Ok(())
    }

    /// `addstr2buff`
    fn add_str(&mut self, h: &mut Heap, s: &[u8]) -> Result<(), Oom> {
        if s.len() <= BUFVFS_54 {
            self.get(h, s.len())?;
            self.space.extend_from_slice(s);
        } else {
            self.clear(h)?;
            self.push(h, s)?;
        }
        Ok(())
    }
}
