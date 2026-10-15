//! The strings PUC makes formatting a message (`lua_pushfstring`,
//! `luaO_pushvfstring`) and joining strings (`luaV_concat`), made in luna
//! too so the count and the string table see what PUC's do. Up to 5.3 each
//! piece of the format becomes a string before the pieces are joined, 5.1
//! and 5.2 in a buffer kept for all concatenations; 5.4 joins them in a
//! buffer on the C stack, making a string of it each time it fills; 5.5
//! grows a block for what does not fit and makes one string at the end.

use crate::numeric::{self, FloatFmt, Num};
use crate::runtime::heap::{Gc, Heap};
use crate::runtime::mem::Oom;
use crate::runtime::table::Dialect;
use crate::runtime::{LuaStr, Value};

/// An argument of a `%` directive.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FmtArg<'a> {
    /// `%s`
    S(&'a [u8]),
    /// `%c`
    C(u8),
    /// `%d`: a C `int`
    D(i64),
    /// `%I`: a `lua_Integer`
    I(i64),
    /// `%f`: a `lua_Number`
    F(f64),
    /// `%p`
    P(usize),
    /// `%U`: a code point, as UTF-8
    U(u32),
}

/// 5.4's `BUFVFS`.
const BUFVFS_54: usize = 60 + 44 + 95;
/// 5.5's `BUFVFS`.
const BUFVFS_55: usize = 60 + 64 + 95;
/// 5.4's `MAXNUMBER2STR`: the room a numeral asks of the buffer.
const MAXNUMBER2STR_54: usize = 44;

#[path = "puc_concat.rs"]
mod concat;

impl Heap {
    fn float_fmt(&self) -> FloatFmt {
        match self.table_dialect {
            Dialect::L51 | Dialect::L52 => FloatFmt::Legacy14,
            Dialect::L53 | Dialect::L54 => FloatFmt::G14,
            Dialect::L55 => FloatFmt::TwoStage55,
        }
    }

    /// The text of a number as PUC writes it (`lua_number2str`,
    /// `tostringbuff`); 5.1 and 5.2 have no integers.
    pub(crate) fn number_text(&self, n: Num) -> String {
        let n = match (self.table_dialect, n) {
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
        match self.table_dialect {
            Dialect::L54 => self.pushfstring_54(fmt, args),
            Dialect::L55 => self.pushfstring_55(fmt, args),
            _ => self.pushfstring_pieces(fmt, args),
        }
    }

    /// 5.1 to 5.3: every piece a string (or a number), joined at the end.
    fn pushfstring_pieces(&mut self, fmt: &[u8], args: &[FmtArg<'_>]) -> Result<Gc<LuaStr>, Oom> {
        let d = self.table_dialect;
        let mut st: Vec<Value> = Vec::new();
        if d == Dialect::L51 {
            st.push(Value::Str(self.try_intern(b"")?));
        }
        let mut rest = fmt;
        let mut args = args.iter();
        let mut n = 0;
        while let Some(e) = rest.iter().position(|&c| c == b'%') {
            st.push(Value::Str(self.try_intern(&rest[..e])?));
            let spec = rest.get(e + 1).copied().unwrap_or(0);
            let v = match (spec, args.next()) {
                (b'%', _) => Value::Str(self.try_intern(b"%")?),
                (_, Some(FmtArg::S(s))) => Value::Str(self.try_intern(s)?),
                (_, Some(&FmtArg::C(c))) => {
                    if d == Dialect::L53 && !(0x20..0x7f).contains(&c) {
                        // a non-printable character is written as its code
                        Value::Str(self.pushfstring(b"<\\%d>", &[FmtArg::D(i64::from(c))])?)
                    } else {
                        Value::Str(self.try_intern(&[c])?)
                    }
                }
                (_, Some(&FmtArg::D(i) | &FmtArg::I(i))) => self.fmt_number(Num::Int(i))?,
                (_, Some(&FmtArg::F(f))) => self.fmt_number(Num::Float(f))?,
                (_, Some(&FmtArg::P(p))) => {
                    let t = crate::vm::cfmt::c_pointer(p);
                    Value::Str(self.try_intern(t.as_bytes())?)
                }
                (_, Some(&FmtArg::U(u))) => {
                    let mut b = [0u8; 8];
                    let l = utf8_esc(&mut b, u);
                    Value::Str(self.try_intern(&b[..l])?)
                }
                (_, None) => Value::Str(self.try_intern(&[b'%', spec])?),
            };
            st.push(v);
            n += 2;
            rest = &rest[(e + 2).min(rest.len())..];
        }
        st.push(Value::Str(self.try_intern(rest)?));
        if d == Dialect::L51 {
            let total = st.len();
            self.puc_concat(&mut st, total)?;
        } else if n > 0 {
            self.puc_concat(&mut st, n + 1)?;
        }
        match st.last() {
            Some(Value::Str(s)) => Ok(*s),
            _ => unreachable!("a concatenation leaves a string"),
        }
    }

    /// A number pushed by `%d`, `%I` or `%f`: 5.1 and 5.2 leave the number
    /// for the concatenation to convert, 5.3 converts it at once.
    fn fmt_number(&mut self, n: Num) -> Result<Value, Oom> {
        let v = match n {
            Num::Int(i) if self.table_dialect >= Dialect::L53 => Value::Int(i),
            Num::Int(i) => Value::Float(i as f64),
            Num::Float(f) => Value::Float(f),
        };
        if self.table_dialect == Dialect::L53 {
            let t = self.number_text(n);
            return Ok(Value::Str(self.try_intern(t.as_bytes())?));
        }
        Ok(v)
    }

    /// 5.4: the pieces gathered in a buffer of `BUFVFS` bytes; each time it
    /// fills (and at the end) its contents become a string, joined to the
    /// one before.
    fn pushfstring_54(&mut self, fmt: &[u8], args: &[FmtArg<'_>]) -> Result<Gc<LuaStr>, Oom> {
        let mut b = Bufvfs {
            space: Vec::with_capacity(BUFVFS_54),
            pushed: None,
        };
        let mut rest = fmt;
        let mut args = args.iter();
        while let Some(e) = rest.iter().position(|&c| c == b'%') {
            b.add_str(self, &rest[..e])?;
            let spec = rest.get(e + 1).copied().unwrap_or(0);
            match (spec, args.next()) {
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
                    let t = crate::vm::cfmt::c_pointer(p);
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
    fn pushfstring_55(&mut self, fmt: &[u8], args: &[FmtArg<'_>]) -> Result<Gc<LuaStr>, Oom> {
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
            match (spec, args.next()) {
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
                    let t = crate::vm::cfmt::c_pointer(p);
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

/// PUC `luaO_utf8esc`: `x` (up to 2^31-1) in extended UTF-8, written to
/// the front of `out`; its length.
pub(crate) fn utf8_esc(out: &mut [u8; 8], mut x: u32) -> usize {
    if x < 0x80 {
        out[0] = x as u8;
        return 1;
    }
    let mut cont = [0u8; 6];
    let mut n = 0;
    let mut mfb: u32 = 0x3f;
    loop {
        cont[n] = 0x80 | (x & 0x3f) as u8;
        n += 1;
        x >>= 6;
        mfb >>= 1;
        if x <= mfb {
            break;
        }
    }
    out[0] = ((!mfb << 1) | x) as u8;
    for (i, &b) in cont[..n].iter().rev().enumerate() {
        out[1 + i] = b;
    }
    n + 1
}
