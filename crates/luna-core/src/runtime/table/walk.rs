//! `next` traversal.

use super::*;

impl Table {
    /// Lua `next`: iterate array part then hash part.
    pub fn next(&self, key: Value) -> Result<Option<(Value, Value)>, TableError> {
        let start = match key {
            Value::Nil => 0,
            k => {
                let k = match k {
                    Value::Float(f) => match f2i_exact(f) {
                        Some(i) => Value::Int(i),
                        None => k,
                    },
                    k => k,
                };
                if let Value::Int(i) = k
                    && i >= 1
                    && (i as u64) <= self.asize() as u64
                {
                    i as usize
                } else {
                    match self.find_node(k) {
                        Some(idx) => self.asize() + idx + 1,
                        None => return Err(TableError::InvalidNext),
                    }
                }
            }
        };
        let atags = self.atags();
        for i in start..self.asize() {
            if atags[i] != raw::NIL {
                return Ok(Some((Value::Int(i as i64 + 1), self.aget(i))));
            }
        }
        let hstart = start.saturating_sub(self.asize());
        for (idx, n) in self.nodes().iter().enumerate().skip(hstart) {
            if !n.val.is_nil() {
                let _ = idx;
                return Ok(Some((n.shown_key(), n.val)));
            }
        }
        Ok(None)
    }
}
