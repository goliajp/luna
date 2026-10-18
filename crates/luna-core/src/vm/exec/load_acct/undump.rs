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
        let d = self.heap.table_dialect;
        let (mut protos, mut closing) = (Vec::new(), Vec::new());
        walk(cl.proto, &mut protos, &mut closing);
        let mut protos = protos.into_iter();
        let mut open: Vec<(Gc<Proto>, [u32; 7])> = Vec::new();
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
                    let size = self.heap.sizes.lua_closure(1);
                    self.heap.acct_ghost(PucKind::LuaClosure, size)?;
                }
                UEv::Closure(_) | UEv::Reclose(_) => {
                    self.acct_main_closure(cl);
                    if let Some(h) = h.as_mut() {
                        h.insert(&mut self.heap, d, PKey::of(Value::Closure(cl)));
                    }
                }
                UEv::Proto => {
                    let p = protos.next().unwrap_or(cl.proto);
                    let hdr = p.as_ptr() as *mut crate::runtime::heap::GcHeader;
                    let size = self.heap.sizes.proto;
                    self.heap.acct_promote_obj(hdr, PucKind::Proto, size);
                    open.push((p, [0; 7]));
                }
                UEv::End => {
                    if let Some((p, arrays)) = open.pop() {
                        p.puc_arrays.set(arrays);
                    }
                }
                UEv::Vec(a, n) => {
                    let b = n as usize * self.elem(a);
                    self.heap.acct_block(0, b);
                    if let Some((_, arrays)) = open.last_mut() {
                        arrays[a as usize] = b as u32;
                    }
                }
                UEv::Buff(n) if n as usize > buff => {
                    let new = (n as usize).max(MIN_BUFFER);
                    self.heap.acct_block(buff, new);
                    buff = new;
                }
                UEv::Buff(_) => {}
                UEv::Str(s) => {
                    self.undump_string(s, open.last().map(|o| o.0))?;
                    if let Some(h) = h.as_mut() {
                        saved += 1;
                        h.insert(&mut self.heap, d, PKey::Int(saved));
                    }
                }
            }
        }
        // `f_parser`'s closure (5.1) and its upvalues; then
        // `luaD_protectedparser` frees the buffer
        if d == Dialect::L51 {
            self.acct_main_closure(cl);
        } else {
            let s = self.heap.sizes;
            for &uv in cl.upvals().iter().take(cl.proto.upvals.len()) {
                let hdr = uv.as_ptr() as *mut crate::runtime::heap::GcHeader;
                self.heap.acct_promote_obj(hdr, PucKind::Upval, s.upval);
            }
        }
        self.heap.acct_block(buff, 0);
        Ok(())
    }

    /// The bytes of one element of a prototype's array `a`.
    fn elem(&self, a: Arr) -> usize {
        let s = self.heap.sizes;
        match a {
            Arr::Code => 4,
            Arr::Protos => s.ptr,
            Arr::Consts => s.tvalue,
            Arr::Lines if matches!(self.heap.table_dialect, Dialect::L54 | Dialect::L55) => 1,
            Arr::Lines => 4,
            Arr::AbsLines => s.abslineinfo,
            Arr::LocVars => s.locvar,
            Arr::Upvals => s.upvaldesc,
        }
    }

    /// A string the undumper makes for the function `p`: a short one is
    /// interned (counted when it is new to the count); a long one is the
    /// string of `p`'s that luna made with those bytes, counted now, or
    /// a record of one luna has none of.
    fn undump_string(&mut self, b: &[u8], p: Option<Gc<Proto>>) -> Result<Value, SyntaxError> {
        let long = b.len() > crate::runtime::string::MAX_SHORT_LEN
            && self.heap.table_dialect != Dialect::L51;
        if !long {
            return Ok(Value::Str(self.heap.try_intern(b)?));
        }
        let uncounted = |s: &Gc<crate::runtime::LuaStr>| {
            s.as_bytes() == b && crate::runtime::heap::is_uncounted(s.as_ptr() as *mut _)
        };
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
                self.heap.acct_promote(s.as_ptr() as *mut _, b.len());
                Ok(Value::Str(s))
            }
            None => {
                let size = self
                    .heap
                    .sizes
                    .string(b.len(), self.heap.table_dialect == Dialect::L55);
                self.heap.acct_ghost(PucKind::LongStr, size)?;
                Ok(Value::Bool(true))
            }
        }
    }
}
