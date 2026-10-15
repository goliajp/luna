//! PUC's `luaV_concat` as far as what it makes goes.

use super::*;

impl Heap {
    /// PUC `luaV_concat(L, total)` over the top `total` values of `st`,
    /// every one a string or a number: what it makes, in its order, and
    /// the one value it leaves in their place.
    pub(crate) fn puc_concat(&mut self, st: &mut Vec<Value>, total: usize) -> Result<(), Oom> {
        let d = self.table_dialect;
        let mut total = total;
        while total > 1 {
            let top = st.len();
            self.tostring_value(&mut st[top - 1])?;
            let Value::Str(y) = st[top - 1] else {
                unreachable!("converted above")
            };
            let mut n = 2;
            if y.is_empty() {
                self.tostring_value(&mut st[top - 2])?;
            } else if d >= Dialect::L52 && matches!(st[top - 2], Value::Str(s) if s.is_empty()) {
                st[top - 2] = Value::Str(y);
            } else {
                let mut tl = y.len();
                n = 1;
                while n < total {
                    self.tostring_value(&mut st[top - n - 1])?;
                    let Value::Str(s) = st[top - n - 1] else {
                        unreachable!("converted above")
                    };
                    tl += s.len();
                    n += 1;
                }
                if d <= Dialect::L52 {
                    self.open_gbuff(tl);
                }
                let parts: Vec<&[u8]> = st[top - n..]
                    .iter()
                    .map(|v| match v {
                        // SAFETY: the strings are held in `st` and no
                        // collection runs while the parts are borrowed
                        Value::Str(s) => unsafe { s.as_bytes_unbounded() },
                        _ => unreachable!("converted above"),
                    })
                    .collect();
                let s = self.try_intern_parts(&parts)?;
                st[top - n] = Value::Str(s);
            }
            st.truncate(top - (n - 1));
            total -= n - 1;
        }
        Ok(())
    }

    /// PUC's `tostring` of a concatenation's operand: a number becomes the
    /// string of it.
    fn tostring_value(&mut self, v: &mut Value) -> Result<(), Oom> {
        let n = match *v {
            Value::Int(i) => Num::Int(i),
            Value::Float(f) => Num::Float(f),
            _ => return Ok(()),
        };
        let t = self.number_text(n);
        *v = Value::Str(self.try_intern(t.as_bytes())?);
        Ok(())
    }
}
