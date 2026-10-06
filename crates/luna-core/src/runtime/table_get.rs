//! Reads: array and hash lookup.

use super::*;

impl Table {
    /// Raw lookup (no `__index` metamethod). Returns `Value::Nil` when
    /// the key is absent. `Value::Nil` and NaN floats return `nil` directly.
    pub fn get(&self, key: Value) -> Value {
        match key {
            Value::Int(i) => self.get_int(i),
            Value::Float(f) => match f2i_exact(f) {
                Some(i) => self.get_int(i),
                None => {
                    if f.is_nan() {
                        Value::Nil
                    } else {
                        self.get_hash(key)
                    }
                }
            },
            Value::Nil => Value::Nil,
            k => self.get_hash(k),
        }
    }

    /// The array slot of integer key `i`, if the array part has one. An
    /// index past `alimit` (only a 5.4 table's can be) raises it, as PUC
    /// 5.4 `luaH_getint` does.
    #[inline(always)]
    pub(crate) fn array_index(&self, i: i64) -> Option<usize> {
        let idx = (i as u64).wrapping_sub(1);
        if idx < u64::from(self.alimit.get()) {
            return Some(idx as usize);
        }
        if idx < self.asize {
            self.alimit.set(i as u32);
            return Some(idx as usize);
        }
        None
    }

    /// Integer-keyed variant of [`Self::get`].
    #[inline]
    pub fn get_int(&self, i: i64) -> Value {
        if let Some(idx) = self.array_index(i) {
            return self.aget(idx);
        }
        self.get_hash(Value::Int(i))
    }

    /// String-keyed variant of [`Self::get`]: interned strings by pointer
    /// (PUC `luaH_getshortstr`), a long string by the general walk when the
    /// pointer walk misses.
    #[inline]
    pub fn get_str(&self, key: crate::runtime::Gc<crate::runtime::string::LuaStr>) -> Value {
        match self.str_slot_by_ptr(key) {
            Some(v) => *v,
            None if key.is_short() => Value::Nil,
            None => self.get_hash(Value::Str(key)),
        }
    }

    pub(super) fn get_hash(&self, k: Value) -> Value {
        match self.find_node(k) {
            Some(idx) => self.nodes()[idx].val,
            None => Value::Nil,
        }
    }

    /// Same logic as [`find_node`] but exposed
    /// to luna-core's recorder so it can capture the slot index for
    /// the table-field IC snapshot. luna-jit reads neither the
    /// `nodes` field nor `Node` directly; only the slot index
    /// crosses the crate boundary (baked into the IR as a `iconst`).
    #[allow(dead_code)]
    pub(crate) fn find_node_idx(&self, k: Value) -> Option<usize> {
        self.find_node(k)
    }

    /// Accessor for the recorder's
    /// `FieldIcSnapshot` capture: read the slot's value's tag byte
    /// for the cached_val_tag field. The recorder needs this to
    /// match the runtime guard the IC emits. Returns None when
    /// `idx >= nodes.len()`.
    #[allow(dead_code)]
    pub(crate) fn node_val_at(&self, idx: usize) -> Option<Value> {
        self.nodes().get(idx).map(|n| n.val)
    }

    /// Accessor for `nodes.len()` so the recorder
    /// can capture the shape-guard's `nodes_len` field without
    /// reaching into the private `nodes` member.
    #[allow(dead_code)]
    pub(crate) fn nodes_capacity(&self) -> usize {
        self.nodes().len()
    }

    /// The node holding key `k`. Interned strings take the pointer-compare
    /// walk here; every other key walks out of line, so the string path
    /// (the common one) does not pay for the general walk's saved registers.
    #[inline]
    pub(super) fn find_node(&self, k: Value) -> Option<usize> {
        #[cfg(feature = "gc-verify")]
        self.verify_find_node_keys(k);
        if self.nodes().is_empty() {
            return None;
        }
        if let Value::Str(s) = k
            && s.is_short()
        {
            return self.find_short_str(s);
        }
        self.find_node_chain(k)
    }

    /// Walk the chain rooted at the key's main position. `nodes` is non-empty.
    #[inline(never)]
    pub(super) fn find_node_chain(&self, k: Value) -> Option<usize> {
        let mut idx = self.main_position(k);
        loop {
            let n = &self.nodes()[idx];
            // Dead-key slots carry a dangling Gc pointer whose memory may
            // have been reallocated to a different live object; raw_eq on
            // such a key can spuriously match the freshly-reused address.
            // Skip the comparison and only follow `next` (PUC `setdeadkey`
            // / `equalkey` short-circuit). 5.5 gc.lua :459-:478 was 12%
            // flaky on this exact path — a swept B-string's slot kept
            // chaining into A's slot, so `a[k] = nil` (k = A_string) hit
            // the dead slot and wrote nil there, leaving A's val untouched.
            if n.key().raw_eq(k) {
                return Some(idx);
            }
            if n.next == NONE {
                return None;
            }
            idx = n.next as usize;
        }
    }

    /// [`Self::find_node`] for an interned (short) string key: two short
    /// strings are equal only if they are the same object, so the chain
    /// walk compares pointers (PUC `luaH_getshortstr`). `nodes` is non-empty.
    #[inline]
    pub(super) fn find_short_str(&self, key: Gc<crate::runtime::string::LuaStr>) -> Option<usize> {
        let mut idx = key.hash() as usize & (self.nodes().len() - 1);
        loop {
            debug_assert!(idx < self.nodes().len());
            // SAFETY: the main position is masked to the node count and
            // every `next` link is a node index written by `insert_new`.
            let n = unsafe { self.nodes().get_unchecked(idx) };
            if n.key_is_str(key) {
                return Some(idx);
            }
            if n.next == NONE {
                return None;
            }
            idx = n.next as usize;
        }
    }
}
