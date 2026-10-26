//! The objects PUC's undumper holds while a binary chunk is read
//! through a reader.

use super::*;

/// The objects shown for a binary chunk's undumper, kept by the load
/// for the next reader call (they stay on its stack meanwhile).
#[derive(Default)]
pub(crate) struct UndumpShown {
    /// 5.1: each prototype being read, by the number the undumper gave it
    pub(super) protos: Vec<(u32, Value)>,
    pub(super) closure: Option<Gc<LuaClosure>>,
    /// where the source its prototype has is in the bytes
    pub(super) source: Option<(usize, usize)>,
    /// the long string being read, and where it starts
    pub(super) long: Option<(usize, Gc<crate::runtime::LuaStr>)>,
    pub(super) long_at: usize,
    /// 5.5: the anchor table, and what is in it
    pub(super) anchor: Option<Gc<Table>>,
    pub(super) anchored_closure: bool,
    pub(super) anchored: usize,
}

impl Vm {
    /// What PUC's undumper holds on the stack at `u`, `src` read: 5.1 the
    /// prototypes of the functions it is reading, 5.2 to 5.4 the main
    /// closure and (5.3, 5.4) a long string whose bytes it is reading,
    /// 5.5 its anchor table with the closure and the strings it saved.
    /// What was shown before is shown again, brought up to date: PUC's
    /// objects stay the same through the load.
    pub(super) fn undump_objects(
        &mut self,
        src: &[u8],
        u: &crate::vm::dump::Undumping,
        shown: &mut UndumpShown,
    ) -> Result<Vec<Value>, LuaError> {
        if self.version == LuaVersion::Lua51 {
            // a prototype shown before is the same while it is open
            let same = shown
                .protos
                .iter()
                .zip(&u.open)
                .take_while(|((id, _), o)| id == *o)
                .count();
            shown.protos.truncate(same);
            for &id in &u.open[same..] {
                let p = self.proto_value()?;
                shown.protos.push((id, p));
            }
            return Ok(shown.protos.iter().map(|&(_, p)| p).collect());
        }
        let closure = match u.closure {
            Some(nups) => Some(self.undump_closure(src, u, nups, shown)?),
            None => None,
        };
        if self.version == LuaVersion::Lua55 {
            let t = match shown.anchor {
                Some(t) => t,
                None => self.reader_table()?,
            };
            shown.anchor = Some(t);
            let put = |vm: &mut Vm, k: Value, v: Value| {
                // SAFETY: a table of luna's own the load holds
                let r = vm.table_op(|vm| unsafe { t.as_mut() }.set(&mut vm.heap, k, v));
                r.map_err(|e| vm.table_error(e))
            };
            if let Some(cl) = closure
                && !shown.anchored_closure
            {
                put(self, Value::Closure(cl), Value::Closure(cl))?;
                shown.anchored_closure = true;
            }
            for &(at, len) in &u.saved[shown.anchored..] {
                let s = Value::Str(self.heap.try_intern(&src[at..at + len])?);
                shown.anchored += 1;
                put(self, Value::Int(shown.anchored as i64), s)?;
            }
            return Ok(vec![Value::Table(t)]);
        }
        let mut out: Vec<Value> = closure.into_iter().map(Value::Closure).collect();
        if let Some(n) = u.long {
            // a string of `n` bytes not read yet: its contents are whatever
            // PUC's memory had. The same while it is read
            let s = match shown.long {
                Some((len, s)) if len == n && u.long_at == shown.long_at => s,
                _ => self.heap.try_intern(&vec![0; n])?,
            };
            (shown.long, shown.long_at) = (Some((n, s)), u.long_at);
            out.push(Value::Str(s));
        }
        Ok(out)
    }

    /// The closure PUC's undumper is filling, of `nups` upvalues: its
    /// function's source and first fields as far as they are read. The
    /// one shown before stays, made up to date, unless 5.2 made another
    /// for the function's upvalues.
    pub(super) fn undump_closure(
        &mut self,
        src: &[u8],
        u: &crate::vm::dump::Undumping,
        nups: u8,
        shown: &mut UndumpShown,
    ) -> Result<Gc<LuaClosure>, LuaError> {
        let [line, last, params, vararg] = u.head.unwrap_or_default();
        if let Some(cl) = shown
            .closure
            .filter(|cl| cl.upvals().len() == usize::from(nups))
        {
            // the source is made once it is read: it can be the whole text
            if shown.source != u.src {
                let source = self.undump_source(src, u)?;
                shown.source = u.src;
                stand_in::update_source(cl.proto, source);
                self.heap.barrier_forward(cl.proto, Value::Str(source));
            }
            stand_in::update(cl.proto, [line, last], params, vararg);
            return Ok(cl);
        }
        let source = self.undump_source(src, u)?;
        shown.source = u.src;
        let proto = stand_in::proto(&mut self.heap, source, [line, last], params, vararg)?;
        let cl = self
            .heap
            .try_new_closure_to_fill(proto, usize::from(nups))?;
        for i in 0..usize::from(nups) {
            let uv = self.heap.try_new_upvalue(UpvalState::Closed(Value::Nil))?;
            self.heap.put_upval(cl, i, uv);
        }
        self.heap.mark_luna_only(cl);
        shown.closure = Some(cl);
        Ok(cl)
    }

    /// The main function's source as far as the undumper has read: none
    /// yet is "=?", as PUC shows it.
    pub(super) fn undump_source(
        &mut self,
        src: &[u8],
        u: &crate::vm::dump::Undumping,
    ) -> Result<Gc<crate::runtime::LuaStr>, LuaError> {
        let name = u
            .src
            .filter(|&(_, len)| len > 0)
            .map_or(&b"=?"[..], |(at, len)| &src[at..at + len]);
        Ok(self.heap.try_intern(name)?)
    }
}
