//! The replay of a new constant (PUC's key for it and where it goes), and
//! the writes into the parser's tables.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::heap::GcHeader;
use crate::runtime::puc::api::{PucCount, PucParse};

impl Vm {
    /// The long names PUC's scanner made that its prototypes keep as the
    /// names of local variables and upvalues: their count moves to the
    /// strings luna's prototypes hold for them (`hold_puc_names`), which
    /// live as long as PUC's. 5.2 keeps one per function (each has its own
    /// table of names), later dialects one per chunk; the scanner's other
    /// copies are garbage.
    pub(super) fn replay_long_names(&mut self, r: &mut Replay, p: Gc<crate::runtime::Proto>) {
        if r.rec.is_some() {
            return;
        }
        let per_chunk = r.d != Dialect::L52;
        let mut moved: Vec<Gc<LuaStr>> = Vec::new();
        let mut protos = vec![p];
        while let Some(p) = protos.pop() {
            for &held in p.puc_names.borrow().iter() {
                if per_chunk && moved.iter().any(|m| m.as_bytes() == held.as_bytes()) {
                    continue;
                }
                if let Some(i) = r.longs.iter().position(|l| l.as_bytes() == held.as_bytes()) {
                    moved.push(held);
                    let ss = r.longs.swap_remove(i);
                    self.heap.acct_move_long(ss, held);
                }
            }
            protos.extend(p.protos.iter().copied());
        }
    }

    /// A new constant (`addk`): its entry under PUC's key, in the table
    /// the dialect keys constants in, and the constant array's growth (5.5
    /// grows the array first).
    pub(super) fn replay_const(
        &mut self,
        r: &mut Replay,
        key: Option<crate::compiler::const_map::ConstKey>,
        v: Value,
    ) -> Result<(), SyntaxError> {
        self.take_scanned_long(r, v);
        // the constant's index, the value its key gets
        let index = r.fs.last().map_or(0, |f| f.a.k.n);
        let k_first = r.d == Dialect::L55;
        if k_first && let Some(f) = r.fs.last_mut() {
            self.heap.parser_push(&mut f.a.k, ParserArray::Consts);
        }
        if let Some(key) = key {
            self.key_const(r, key, v, index)?;
        }
        if !k_first && let Some(f) = r.fs.last_mut() {
            self.heap.parser_push(&mut f.a.k, ParserArray::Consts);
        }
        // the constant in the prototype (`luaC_barrier`)
        if let Value::Str(cs) = v
            && r.rec.is_none()
        {
            r.kstr.push(cs);
            self.proto_barrier(r, r.fs.len().wrapping_sub(1), cs.hdr());
        }
        Ok(())
    }

    /// A long string constant is the string the scanner made (luna's
    /// compiler made its own): the count moves to luna's.
    fn take_scanned_long(&mut self, r: &mut Replay, v: Value) {
        if let Value::Str(cs) = v
            && let Some(i) = r.longs.iter().position(|l| l.as_bytes() == cs.as_bytes())
        {
            let ss = r.longs.swap_remove(i);
            if r.rec.is_none() {
                self.heap.acct_move_long(ss, cs);
            }
        }
    }

    /// Constant `v` (luna's key `key`, index `index`) entered in the table
    /// the dialect keys constants in: 5.3 and 5.4 the load's, the others
    /// the function's.
    fn key_const(
        &mut self,
        r: &mut Replay,
        key: crate::compiler::const_map::ConstKey,
        v: Value,
        index: usize,
    ) -> Result<(), SyntaxError> {
        let id = match r.d {
            Dialect::L53 | Dialect::L54 => r.h.as_ref().map(|t| t.id),
            _ => r.fs.last().and_then(|f| f.h.as_ref()).map(|t| t.id),
        };
        let Some(id) = id else {
            return Ok(());
        };
        let (k, kv) = self.puc_const_key(r, id, key, v)?;
        let t = match r.d {
            Dialect::L53 | Dialect::L54 => r.h.as_mut(),
            _ => r.fs.last_mut().and_then(|f| f.h.as_mut()),
        };
        // a string is an object of PUC's; nil is keyed by the table itself
        let obj = match kv {
            PVal::V(Value::Str(s)) => Some(s.hdr()),
            _ => None,
        };
        if let Some(t) = t {
            t.insert(&mut self.heap, r.d, k, obj)?;
        }
        r.note(id, kv, Some(PVal::V(Value::Int(index as i64))));
        Ok(())
    }

    /// The key PUC's `addk` uses for constant `v` (luna's key `key`) in
    /// table number `t`, as a key and as the value it is.
    fn puc_const_key(
        &mut self,
        r: &mut Replay,
        t: u32,
        key: crate::compiler::const_map::ConstKey,
        v: Value,
    ) -> Result<(PKey, PVal), SyntaxError> {
        let k = self.puc_const_value(r, t, key, v)?;
        Ok(match k {
            PVal::V(Value::Str(s)) => (PKey::of_str(s, &mut r.long_keys), k),
            PVal::V(v) => (PKey::of(v), k),
            PVal::Tab(t) => (PKey::Tab(t), k),
            PVal::Closure => unreachable!("no constant is keyed by the closure"),
        })
    }

    /// [`Vm::puc_const_key`]'s key as a value: nil is keyed by the table.
    fn puc_const_value(
        &mut self,
        r: &mut Replay,
        t: u32,
        key: crate::compiler::const_map::ConstKey,
        v: Value,
    ) -> Result<PVal, SyntaxError> {
        use crate::compiler::const_map::ConstKey as K;
        let num = |v: Value| match v {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            _ => None,
        };
        Ok(PVal::V(match (r.d, v) {
            (_, Value::Str(_)) | (_, Value::Bool(_)) => v,
            // nil keys by the table itself
            (_, Value::Nil) => return Ok(PVal::Tab(t)),
            (Dialect::L51, v) => Value::Float(num(v).unwrap_or(0.0) + 0.0),
            // 5.2 keys a zero or a nan by a string of its bytes
            (Dialect::L52, v) => {
                let f = num(v).unwrap_or(0.0);
                if f == 0.0 || f.is_nan() {
                    Value::Str(self.heap.try_intern(&f.to_ne_bytes())?)
                } else {
                    Value::Float(f)
                }
            }
            (Dialect::L53, Value::Int(i)) => {
                Value::LightUserdata(std::ptr::without_provenance(i as usize))
            }
            // 5.5 keys every zero of a function by the function
            (_, _) if key == K::Zero => Value::LightUserdata(std::ptr::without_provenance(
                usize::MAX - r.fs.last().map_or(0, |f| f.shape),
            )),
            (_, _) => match key {
                K::Int(i) => Value::Int(i),
                K::Float(b) => Value::Float(f64::from_bits(b)),
                _ => v,
            },
        }))
    }

    /// 5.5 `luaD_anchorobj`: an object keyed in the chunk's table (parser
    /// table number `t` with its table of PUC's, or the main closure).
    pub(super) fn anchor_in_h(
        &mut self,
        r: &mut Replay,
        t: Option<(u32, Option<Gc<GcHeader>>)>,
    ) -> Result<(), SyntaxError> {
        let (key, obj) = match t {
            Some((t, tab)) => (PKey::Tab(t), tab),
            None => {
                r.anchors += 1;
                let cl = r.cl.map(|c| c.hdr());
                (PKey::Ptr(r.anchors), cl)
            }
        };
        if let Some(h) = r.h.as_mut() {
            h.insert(&mut self.heap, r.d, key, obj)?;
            let id = h.id;
            let obj = t.map_or(PVal::Closure, |t| PVal::Tab(t.0));
            r.note(id, obj, Some(obj));
        }
        Ok(())
    }

    /// The scanner's string `s` keyed in parser table `t` (`luaH_set` on a
    /// new key; one already there allocates nothing).
    pub(super) fn set_str_in(
        &mut self,
        t: &mut PTab,
        d: Dialect,
        s: Gc<LuaStr>,
        k: PKey,
    ) -> Result<(), crate::runtime::mem::Oom> {
        if crate::runtime::gc_hooks::TRACE && !t.contains(k) {
            self.heap.trace_line(format_args!(
                "K {} {} {:08x}",
                t.id,
                String::from_utf8_lossy(s.as_bytes()).escape_debug(),
                s.hash()
            ));
        }
        t.insert(&mut self.heap, d, k, Some(s.hdr()))
    }
}
