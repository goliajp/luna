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

impl Table {
    /// How many keys sit off their main position, moved there by a
    /// collision.
    #[cfg(feature = "gc-trace")]
    pub(crate) fn keys_off_main(&self) -> usize {
        self.nodes()
            .iter()
            .enumerate()
            .filter(|(i, n)| !n.val.is_nil() && self.main_position(n.key()) != *i)
            .count()
    }

    /// Lay this table out again as PUC's `luaL_openlibs` left the library
    /// table it stands for: its parts made anew, the resizes and string
    /// keys of `ops` done in their order, then its other entries. Returns
    /// the keys of `ops` it does not hold.
    pub(crate) fn replay_layout(
        &mut self,
        heap: &mut Heap,
        ops: &[crate::runtime::puc_libs::LibOp],
    ) -> Result<Vec<&'static str>, TableError> {
        use crate::runtime::puc_libs::LibOp;
        let mut entries = Vec::new();
        let mut k = Value::Nil;
        while let Some((nk, v)) = self.next(k)? {
            entries.push((nk, v));
            k = nk;
        }
        self.free_parts(heap.mem());
        let mut missing = Vec::new();
        for op in ops {
            match *op {
                LibOp::R(a, h) => {
                    self.resize(heap, a as usize, h as usize)?;
                    // what goes in the array part goes in at once (the
                    // registry's thread and globals), before a later
                    // rehash would count it
                    let mut i = 0;
                    while i < entries.len() {
                        match entries[i].0 {
                            Value::Int(n) if n >= 1 && n <= i64::from(a) => {
                                let (k, v) = entries.remove(i);
                                self.set(heap, k, v)?;
                            }
                            _ => i += 1,
                        }
                    }
                }
                LibOp::K(name) => {
                    let at = entries.iter().position(
                        |(k, _)| matches!(k, Value::Str(s) if s.as_bytes() == name.as_bytes()),
                    );
                    match at {
                        Some(i) => {
                            let (k, v) = entries.remove(i);
                            self.set(heap, k, v)?;
                        }
                        None => missing.push(name),
                    }
                }
            }
        }
        for (k, v) in entries {
            self.set(heap, k, v)?;
        }
        Ok(missing)
    }
}
