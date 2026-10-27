//! Counting a binary chunk as PUC's undumper allocates it. luna's undump
//! made its objects without counting; the steps PUC takes are read again
//! from the chunk, and each counts the object of luna's that stands for
//! PUC's, or a record of one luna does not have.

use super::*;
use crate::runtime::puc::api::{PucCount, PucLoads, PucParse};
use crate::runtime::puc::fmt::FmtArg;
use crate::vm::dump::{Arr, UEv};

/// `LUA_MINBUFFER`: the least a load buffer grows to.
const MIN_BUFFER: usize = 32;

impl Vm {
    /// Count the load of the binary chunk `bytes`, which luna loaded as
    /// `cl` without counting.
    pub(in crate::vm::exec) fn acct_undump(
        &mut self,
        bytes: &[u8],
        cl: Gc<LuaClosure>,
    ) -> Result<(), SyntaxError> {
        let Some(evs) = crate::vm::dump::undump_events(bytes) else {
            return Ok(());
        };
        let d = self.heap.dialect();
        let (mut protos, mut closing) = (Vec::new(), Vec::new());
        walk(cl.proto, &mut protos, &mut closing);
        let mut protos = protos.into_iter();
        let mut open: Vec<Gc<Proto>> = Vec::new();
        let reclose = evs.iter().any(|e| matches!(e, UEv::Reclose(_)));
        // 5.5's `f_parser` anchors what the load makes in a table
        let mut h = match d {
            Dialect::L55 => Some(PTab::new(&mut self.heap, 0, 16, true)?),
            _ => None,
        };
        let (mut buff, mut saved) = (0, 0);
        for ev in evs {
            match ev {
                // 5.2 makes a closure of one upvalue first, and another
                // when the function has a different number
                UEv::Closure(_) if reclose => {
                    self.heap.ghost_obj(PucObj::LuaClosure(1))?;
                }
                UEv::Closure(_) | UEv::Reclose(_) => {
                    self.acct_main_closure(cl);
                    if let Some(h) = h.as_mut() {
                        h.insert(&mut self.heap, d, PKey::of(Value::Closure(cl)), None)?;
                    }
                }
                UEv::Proto => {
                    let p = protos.next().unwrap_or(cl.proto);
                    let hdr = p.hdr();
                    self.heap.promote_obj(hdr, PucObj::Proto);
                    // its arrays are kept as they are counted below
                    p.puc_arrays.set([0; 7]);
                    open.push(p);
                }
                UEv::End => {
                    open.pop();
                }
                UEv::Vec(a, n) => {
                    let b = self.heap.parser_bytes(parser_array(a), n as usize);
                    self.heap.acct_block(0, b);
                    if let Some(p) = open.last() {
                        let mut arrays = p.puc_arrays.get();
                        arrays[a as usize] = b as u32;
                        p.puc_arrays.set(arrays);
                    }
                }
                UEv::Buff(n) if n as usize > buff => {
                    let new = (n as usize).max(MIN_BUFFER);
                    self.heap.acct_block(buff, new);
                    buff = new;
                }
                UEv::Buff(_) => {}
                UEv::Str(s) => {
                    self.undump_string(s, open.last().copied())?;
                    if let Some(h) = h.as_mut() {
                        saved += 1;
                        h.insert(&mut self.heap, d, PKey::Int(saved), None)?;
                    }
                }
            }
        }
        // `f_parser`'s closure (5.1) and its upvalues; then
        // `luaD_protectedparser` frees the buffer
        if d == Dialect::L51 {
            self.acct_main_closure(cl);
        } else {
            for &uv in cl.upvals().iter().take(cl.proto.upvals.len()) {
                self.heap.promote_obj(uv.hdr(), PucObj::Upval);
            }
        }
        self.heap.acct_block(buff, 0);
        Ok(())
    }

    /// Count the load of the binary chunk `bytes` that luna refused (cut
    /// short, or bad past some point): what PUC's undumper made before it
    /// met the end of the bytes or the bad one, all of it garbage now.
    /// Its message `e`, for the chunk `chunkname`, is made last.
    pub(in crate::vm::exec) fn acct_failed_undump(
        &mut self,
        bytes: &[u8],
        chunkname: &[u8],
        e: &SyntaxError,
    ) -> Result<(), SyntaxError> {
        let mut u = crate::vm::dump::Undumping::new(true);
        u.feed(bytes);
        let Some(mut evs) = u.events_so_far(bytes) else {
            return Ok(());
        };
        let refused = self.refused_block(&mut evs, u.cut_long);
        let d = self.heap.dialect();
        let mut h = match d {
            Dialect::L55 => Some(PTab::new(&mut self.heap, 0, 16, true)?),
            _ => None,
        };
        let (mut buff, mut saved) = (0, 0);
        for (i, ev) in evs.iter().enumerate() {
            match *ev {
                UEv::Closure(n) | UEv::Reclose(n) => {
                    self.heap.ghost_obj(PucObj::LuaClosure(n as usize))?;
                    if let Some(h) = h.as_mut() {
                        h.insert(&mut self.heap, d, PKey::Ptr(1), None)?;
                    }
                }
                // a prototype and the arrays read for it, freed with it
                UEv::Proto => {
                    let arrays = self.arrays_of(&evs[i + 1..]);
                    self.heap.ghost_obj(PucObj::ProtoWithArrays(arrays))?;
                }
                UEv::End | UEv::Vec(..) => {}
                UEv::Buff(n) if n as usize > buff => {
                    let new = (n as usize).max(MIN_BUFFER);
                    self.heap.acct_block(buff, new);
                    buff = new;
                }
                UEv::Buff(_) => {}
                UEv::Str(s) => {
                    self.undump_string(s, None)?;
                    if let Some(h) = h.as_mut() {
                        saved += 1;
                        h.insert(&mut self.heap, d, PKey::Int(saved), None)?;
                    }
                }
            }
        }
        self.end_failed_undump(u.cut_long.filter(|_| refused.is_none()), buff)?;
        match refused {
            Some((_, too_big)) => Err(self.refused_block_error(too_big)),
            None => self.undump_error(chunkname, e),
        }
    }

    /// The end of a refused binary chunk's load: the cut long string of
    /// `len` bytes made (before its bytes are read), the buffer of `buff`
    /// bytes freed.
    fn end_failed_undump(&mut self, len: Option<usize>, buff: usize) -> Result<(), SyntaxError> {
        if let Some(len) = len {
            self.heap.ghost_obj(PucObj::Str { len, long: true })?;
        }
        self.heap.acct_block(buff, 0);
        Ok(())
    }

    /// 5.1 to 5.3 make the message of a refused binary chunk with
    /// `luaO_pushfstring`, a string for each piece; it is the load's error.
    fn undump_error(&mut self, chunkname: &[u8], e: &SyntaxError) -> Result<(), SyntaxError> {
        let (fmt, suffix): (&[u8], &[u8]) = match self.version {
            LuaVersion::Lua51 => (b"%s: %s in precompiled chunk", b" in precompiled chunk"),
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                (b"%s: %s precompiled chunk", b" precompiled chunk")
            }
            _ => return Ok(()),
        };
        // `luaU_undump`'s name of the chunk
        let name: &[u8] = match chunkname.first() {
            Some(b'@' | b'=') => &chunkname[1..],
            Some(0x1b) => b"binary string",
            _ => chunkname,
        };
        let why = e
            .msg
            .strip_prefix(name)
            .and_then(|m| m.strip_prefix(b": "))
            .and_then(|m| m.strip_suffix(suffix));
        let Some(why) = why else {
            return Ok(());
        };
        let args = [FmtArg::S(name), FmtArg::S(why)];
        let m = self
            .pushfstring(fmt, &args)
            .map_err(|_| SyntaxError::memory())?;
        self.heap.set_load_msg(m);
        Ok(())
    }

    /// PUC's bytes of the arrays the events read for the prototype they
    /// start in, up to its end (its inner functions' are theirs).
    fn arrays_of(&self, evs: &[UEv]) -> usize {
        let mut depth = 0usize;
        let mut bytes = 0;
        for ev in evs {
            match *ev {
                UEv::Proto => depth += 1,
                UEv::End if depth == 0 => break,
                UEv::End => depth -= 1,
                UEv::Vec(a, n) if depth == 0 => {
                    bytes += self.heap.parser_bytes(parser_array(a), n as usize)
                }
                _ => {}
            }
        }
        bytes
    }

    /// A string the undumper makes for the function `p`: a short one is
    /// interned (counted when it is new to the count); a long one is the
    /// string of `p`'s that luna made with those bytes, counted now, or
    /// a record of one luna has none of.
    fn undump_string(&mut self, b: &[u8], p: Option<Gc<Proto>>) -> Result<Value, SyntaxError> {
        let long =
            b.len() > crate::runtime::string::MAX_SHORT_LEN && self.heap.dialect() != Dialect::L51;
        if !long {
            return Ok(Value::Str(self.heap.try_intern(b)?));
        }
        let uncounted =
            |s: &Gc<crate::runtime::LuaStr>| s.as_bytes() == b && s.hdr().is_uncounted();
        let found = p.and_then(|p| {
            let consts = p.consts.iter().filter_map(|v| match v {
                Value::Str(s) => Some(*s),
                _ => None,
            });
            let names = p.puc_names.borrow().clone();
            std::iter::once(p.source)
                .chain(consts)
                .chain(names)
                .find(|s| uncounted(s))
        });
        match found {
            Some(s) => {
                self.heap.acct_promote(s, b.len());
                Ok(Value::Str(s))
            }
            None => {
                self.heap.ghost_obj(PucObj::Str {
                    len: b.len(),
                    long: true,
                })?;
                Ok(Value::Bool(true))
            }
        }
    }
}

/// The parser's array an undumped array of `a` is.
pub(super) fn parser_array(a: Arr) -> ParserArray {
    match a {
        Arr::Code => ParserArray::Code,
        Arr::Protos => ParserArray::Protos,
        Arr::Consts => ParserArray::Consts,
        Arr::Lines => ParserArray::LineInfo,
        Arr::AbsLines => ParserArray::AbsLines,
        Arr::LocVars => ParserArray::LocVars,
        Arr::Upvals => ParserArray::Upvals,
    }
}
