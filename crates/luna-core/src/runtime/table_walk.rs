//! Length (border search) and `next` traversal.

use super::*;

impl Table {
    /// A border: `n` where `t[n]` is non-nil and `t[n+1]` is nil (PUC `luaH_getn`).
    /// This is Lua `#` semantics, not a container size — an `is_empty`
    /// counterpart would be meaningless.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> i64 {
        let asize = self.asize();
        if self.acount == self.aprefix && (self.aprefix as usize) < asize {
            return self.aprefix as i64;
        }
        let atags = self.atags();
        if asize > 0 && atags[asize - 1] == raw::NIL {
            // binary search inside the array part
            let (mut lo, mut hi) = (0usize, asize);
            while hi - lo > 1 {
                let m = lo + (hi - lo) / 2;
                if atags[m - 1] == raw::NIL {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            return lo as i64;
        }
        if self.nodes().is_empty() {
            return asize as i64;
        }
        // array is full (or absent): unbound search through the hash part
        let mut lo = asize as i64;
        let mut hi = lo + 1;
        while !self.get_int(hi).is_nil() {
            lo = hi;
            match hi.checked_mul(2) {
                Some(n) => hi = n,
                None => {
                    // pathological sparse keys (the doubling overflowed): scan
                    // linearly from 1 for the first border, as PUC's
                    // unbound_search does — finds a small border fast instead of
                    // returning the huge one.
                    let mut i = 1i64;
                    while !self.get_int(i).is_nil() {
                        i += 1;
                    }
                    return i - 1;
                }
            }
        }
        while hi - lo > 1 {
            let m = lo + (hi - lo) / 2;
            if self.get_int(m).is_nil() {
                hi = m;
            } else {
                lo = m;
            }
        }
        lo
    }

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
