//! Counting a binary chunk as PUC's undumper allocates it. luna's undump
//! made its objects without counting; the steps PUC takes are read again
//! from the chunk, and each counts the object of luna's that stands for
//! PUC's, or a record of one luna does not have.

use super::*;
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
                    let size = self
                        .heap
                        .puc_size(crate::runtime::puc::PucObj::LuaClosure(1));
                    self.heap.acct_ghost(PucKind::LuaClosure, size)?;
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
                    let size = self.heap.puc_size(crate::runtime::puc::PucObj::Proto);
                    self.heap.acct_promote_obj(hdr, PucKind::Proto, size);
                    // its arrays are kept as they are counted below
                    p.puc_arrays.set([0; 7]);
                    open.push(p);
                }
                UEv::End => {
                    open.pop();
                }
                UEv::Vec(a, n) => {
                    let b = self.heap.parser_bytes(
                        match a {
                            Arr::Code => ParserArray::Code,
                            Arr::Protos => ParserArray::Protos,
                            Arr::Consts => ParserArray::Consts,
                            Arr::Lines => ParserArray::LineInfo,
                            Arr::AbsLines => ParserArray::AbsLines,
                            Arr::LocVars => ParserArray::LocVars,
                            Arr::Upvals => ParserArray::Upvals,
                        },
                        n as usize,
                    );
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
            let upval = self.heap.puc_size(crate::runtime::puc::PucObj::Upval);
            for &uv in cl.upvals().iter().take(cl.proto.upvals.len()) {
                let hdr = uv.hdr();
                self.heap.acct_promote_obj(hdr, PucKind::Upval, upval);
            }
        }
        self.heap.acct_block(buff, 0);
        Ok(())
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
                let size = self.heap.puc_size(crate::runtime::puc::PucObj::Str {
                    len: b.len(),
                    long: true,
                });
                self.heap.acct_ghost(PucKind::LongStr, size)?;
                Ok(Value::Bool(true))
            }
        }
    }
}
