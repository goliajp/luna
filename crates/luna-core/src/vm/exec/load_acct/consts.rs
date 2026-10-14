//! The replay of a new constant (PUC's key for it and where it goes), and
//! the writes into the parser's tables.

use super::*;

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
        let s = self.heap.sizes;
        let k_first = r.d == Dialect::L55;
        if k_first && let Some(f) = r.fs.last_mut() {
            f.k.push(&mut self.heap, s.tvalue);
        }
        if let Some(key) = key {
            let t = match r.d {
                Dialect::L53 | Dialect::L54 => r.h,
                _ => r.fs.last().and_then(|f| f.h),
            };
            if let Some(t) = t {
                let k = self.puc_const_key(r, t, key, v)?;
                self.set_in(t, k, Value::Int(0))?;
            }
        }
        if !k_first && let Some(f) = r.fs.last_mut() {
            f.k.push(&mut self.heap, s.tvalue);
        }
        Ok(())
    }

    /// The key PUC's `addk` uses for constant `v` (luna's key `key`) in
    /// table `t`.
    fn puc_const_key(
        &mut self,
        r: &Replay,
        t: Gc<Table>,
        key: crate::compiler::const_map::ConstKey,
        v: Value,
    ) -> Result<Value, SyntaxError> {
        use crate::compiler::const_map::ConstKey as K;
        let num = |v: Value| match v {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            _ => None,
        };
        Ok(match (r.d, v) {
            (_, Value::Str(_) | Value::Bool(_)) => v,
            // nil keys by the table itself
            (_, Value::Nil) => Value::Table(t),
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
        })
    }

    /// 5.5 `luaD_anchorobj`: an object keyed in the chunk's table (`t`, or
    /// one luna has no object for).
    pub(super) fn anchor_in_h(
        &mut self,
        r: &mut Replay,
        t: Option<Gc<Table>>,
    ) -> Result<(), SyntaxError> {
        let key = match t {
            Some(t) => Value::Table(t),
            None => {
                r.anchors += 1;
                Value::LightUserdata(r.anchors as *const ())
            }
        };
        match r.h {
            Some(h) => self.set_in(h, key, key),
            None => Ok(()),
        }
    }

    /// `t[k] = v` in a parser table, when `k` is not there yet (PUC
    /// `luaH_set` on a new key; a key already there allocates nothing).
    pub(super) fn set_in(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), SyntaxError> {
        if !t.get(k).is_nil() {
            return Ok(());
        }
        #[cfg(feature = "gc-trace")]
        if let Value::Str(s) = k {
            self.heap.trace.line(format_args!(
                "K {:p} {} {:08x}",
                t.as_ptr(),
                String::from_utf8_lossy(s.as_bytes()).escape_debug(),
                s.hash()
            ));
        }
        self.store_in(t, k, v)
    }

    /// `t[k] = v` in a parser table.
    pub(super) fn store_in(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), SyntaxError> {
        // SAFETY: parser tables are made for this load and held only by
        // the replay
        let t = unsafe { t.as_mut() };
        t.set(&mut self.heap, k, v).map_err(table_err)
    }
}
