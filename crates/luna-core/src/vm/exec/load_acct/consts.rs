//! The replay of a new constant (PUC's key for it and where it goes), and
//! the writes into the parser's tables.

use super::*;
use crate::runtime::LuaStr;

impl Vm {
    /// A new constant (`addk`): its entry under PUC's key, in the table
    /// the dialect keys constants in, and the constant array's growth (5.5
    /// grows the array first).
    pub(super) fn replay_const(
        &mut self,
        r: &mut Replay,
        key: Option<crate::compiler::const_map::ConstKey>,
        v: Value,
    ) -> Result<(), SyntaxError> {
        // a long string constant is the string the scanner made (luna's
        // compiler made its own)
        if let Value::Str(cs) = v
            && let Some(i) = r.longs.iter().position(|l| l.as_bytes() == cs.as_bytes())
        {
            let ss = r.longs.swap_remove(i);
            self.heap.acct_move_long(ss, cs);
        }
        let s = self.heap.sizes;
        let k_first = r.d == Dialect::L55;
        if k_first && let Some(f) = r.fs.last_mut() {
            f.k.push(&mut self.heap, s.tvalue);
        }
        if let Some(key) = key {
            let id = match r.d {
                Dialect::L53 | Dialect::L54 => r.h.as_ref().map(|t| t.id),
                _ => r.fs.last().and_then(|f| f.h.as_ref()).map(|t| t.id),
            };
            if let Some(id) = id {
                let k = self.puc_const_key(r, id, key, v)?;
                let t = match r.d {
                    Dialect::L53 | Dialect::L54 => r.h.as_mut(),
                    _ => r.fs.last_mut().and_then(|f| f.h.as_mut()),
                };
                if let Some(t) = t {
                    t.insert(&mut self.heap, r.d, k);
                }
            }
        }
        if !k_first && let Some(f) = r.fs.last_mut() {
            f.k.push(&mut self.heap, s.tvalue);
        }
        Ok(())
    }

    /// The key PUC's `addk` uses for constant `v` (luna's key `key`) in
    /// table number `t`.
    fn puc_const_key(
        &mut self,
        r: &mut Replay,
        t: u32,
        key: crate::compiler::const_map::ConstKey,
        v: Value,
    ) -> Result<PKey, SyntaxError> {
        use crate::compiler::const_map::ConstKey as K;
        let num = |v: Value| match v {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            _ => None,
        };
        Ok(PKey::of(match (r.d, v) {
            (_, Value::Str(s)) => return Ok(PKey::of_str(s, &mut r.long_keys)),
            (_, Value::Bool(_)) => v,
            // nil keys by the table itself
            (_, Value::Nil) => return Ok(PKey::Tab(t)),
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
            (Dialect::L53, Value::Int(i)) => Value::LightUserdata(i as usize as *const ()),
            // 5.5 keys every zero of a function by the function
            (_, _) if key == K::Zero => {
                Value::LightUserdata((usize::MAX - r.fs.last().map_or(0, |f| f.shape)) as *const ())
            }
            (_, _) => match key {
                K::Int(i) => Value::Int(i),
                K::Float(b) => Value::Float(f64::from_bits(b)),
                _ => v,
            },
        }))
    }

    /// 5.5 `luaD_anchorobj`: an object keyed in the chunk's table (parser
    /// table number `t`, or one luna has no object for).
    pub(super) fn anchor_in_h(&mut self, r: &mut Replay, t: Option<u32>) {
        let key = match t {
            Some(t) => PKey::Tab(t),
            None => {
                r.anchors += 1;
                PKey::Ptr(r.anchors)
            }
        };
        if let Some(h) = r.h.as_mut() {
            h.insert(&mut self.heap, r.d, key);
        }
    }

    /// The scanner's string `s` keyed in parser table `t` (`luaH_set` on a
    /// new key; one already there allocates nothing).
    pub(super) fn set_str_in(
        &mut self,
        t: &mut PTab,
        d: Dialect,
        #[cfg_attr(not(feature = "gc-trace"), allow(unused_variables))] s: Gc<LuaStr>,
        k: PKey,
    ) {
        #[cfg(feature = "gc-trace")]
        if !t.contains(k) {
            self.heap.trace.line(format_args!(
                "K {} {} {:08x}",
                t.id,
                String::from_utf8_lossy(s.as_bytes()).escape_debug(),
                s.hash()
            ));
        }
        t.insert(&mut self.heap, d, k);
    }
}
