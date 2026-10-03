//! Writes: storing into existing slots and inserting new keys.

use super::*;

impl Table {
    /// Insert / update `(key, val)`. `heap` is used to credit any internal
    /// Box growth (rehash) to `heap.bytes` so the counter stays in sync with
    /// real memory; `free_obj` subtracts `internal_bytes()` on the way out.
    /// A read-only table refuses the write with [`TableError::ReadOnly`].
    pub fn set(&mut self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError> {
        if self.is_readonly() {
            return Err(TableError::ReadOnly);
        }
        self.set_inlined(heap, key, val)
    }

    /// [`Self::set`] without the read-only test, for compiled code that
    /// has made it already: the trace JIT's store helpers test it together
    /// with the metatable, and the method JIT's only reach tables a call
    /// tested on entry or made itself. Not part of the supported API.
    #[doc(hidden)]
    pub fn set_unguarded(
        &mut self,
        heap: &mut Heap,
        key: Value,
        val: Value,
    ) -> Result<(), TableError> {
        self.set_inlined(heap, key, val)
    }

    /// [`Self::set_int`] without the read-only test; see
    /// [`Self::set_unguarded`].
    #[doc(hidden)]
    pub fn set_int_unguarded(
        &mut self,
        heap: &mut Heap,
        i: i64,
        val: Value,
    ) -> Result<(), TableError> {
        self.set_norm(heap, Value::Int(i), val)
    }

    /// [`Self::set`] for the interpreter's own write path, which has
    /// checked [`Self::is_readonly`] itself. `set` is left to the
    /// compiler's judgement: marking it `#[inline]` made the JIT's
    /// table-store helpers ~10% slower on aarch64.
    #[inline]
    pub(crate) fn set_inlined(
        &mut self,
        heap: &mut Heap,
        key: Value,
        val: Value,
    ) -> Result<(), TableError> {
        let k = match key {
            Value::Float(f) if f == 0.0 && f.is_sign_negative() && heap.signed_zero_keys => {
                return self.set_neg_zero(heap, val);
            }
            key => normalize_set_key(key)?,
        };
        self.set_norm(heap, k, val)
    }

    /// `t[-0] = val` under 5.1/5.2: the key is 0, and a key that is new
    /// keeps the sign it was given (PUC stores the key value as is).
    #[cold]
    #[inline(never)]
    pub(super) fn set_neg_zero(&mut self, heap: &mut Heap, val: Value) -> Result<(), TableError> {
        let fresh = self.find_node(Value::Int(0)).is_none();
        self.set_norm(heap, Value::Int(0), val)?;
        if fresh {
            self.mark_neg_zero();
        }
        Ok(())
    }

    pub(super) fn mark_neg_zero(&mut self) {
        if let Some(i) = self.find_node(Value::Int(0)) {
            self.nodes_mut()[i].neg_zero = true;
        }
    }

    /// PUC `luaV_fastset` / `luaV_finishfastset` analogue: single-walk
    /// in-place update for an existing key. Returns `true` iff `key` is
    /// present with a non-nil value and the slot was overwritten with
    /// `val`. Returns `false` when the key is absent, the slot holds nil,
    /// or the key normalisation rejects it — the caller is then expected
    /// to run the `__newindex` chain or fall back to `set` for the raw
    /// insert — and when the table is read-only.
    ///
    /// Collapses the SetField hot path from two hash-chain walks
    /// (`get` + `set`) to one. The `__newindex` invariant ("fires iff
    /// `get` would have returned nil") is preserved because this method
    /// writes only when the existing slot is non-nil — the exact set the
    /// prior `tb.get(key).is_nil()` gate already excluded from
    /// `__newindex` eligibility. See
    /// semantics check.
    ///
    /// The caller is responsible for firing `Heap::barrier_back` after a
    /// `true` return (same contract as the surrounding `raw_set`
    /// wrapper).
    #[inline]
    pub fn try_set_existing(&mut self, key: Value, val: Value) -> bool {
        !self.is_readonly() && self.set_existing_raw(key, val)
    }

    /// [`Self::try_set_existing`] for a caller that has checked
    /// [`Self::is_readonly`] itself (the interpreter's stores test it with
    /// the write barrier, `Heap::store_barrier`).
    #[inline]
    pub(crate) fn set_existing_raw(&mut self, key: Value, val: Value) -> bool {
        let k = match normalize_set_key(key) {
            Ok(k) => k,
            Err(_) => return false,
        };
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            let idx = i as usize - 1;
            // SAFETY: `idx < self.asize()` is guarded by the conditional
            // above, mirroring the bound on `aget`/`aset`.
            let tag = unsafe { *self.atags().get_unchecked(idx) };
            if tag != raw::NIL {
                // Nil-val on a live slot must follow the same tombstone
                // discipline as `set_norm` — routed through
                // `clear_existing_slot`.
                if val.is_nil() {
                    self.clear_existing_slot(k);
                } else {
                    self.aset(idx, val);
                }
                return true;
            }
            // Array slot present-but-nil → __newindex eligible: do NOT
            // write. Caller falls through to the metamethod chain.
            return false;
        }
        if let Some(idx) = self.find_node(k)
            && !self.nodes()[idx].val.is_nil()
        {
            if val.is_nil() {
                self.clear_existing_slot(k);
            } else {
                self.nodes_mut()[idx].val = val;
            }
            return true;
        }
        false
    }

    /// Shared "live with val=Nil is illegal" tombstone routine for the
    /// two write entry points (`set_norm` and `try_set_existing`). The
    /// slot must already be known live (array slot inside `asize()` /
    /// node returned by `find_node`).
    ///
    /// Chain-world today:
    ///   - array slot → `aset(_, Nil)` clears the atag, so `next()`'s
    ///     `tag != raw::NIL` filter skips the slot.
    ///   - node slot  → soft tombstone (key kept, `val = Nil`); chain
    ///     `next()` filter `!n.val.is_nil()` skips it, and `find_node`
    ///     still routes a future re-insert into the same slot without
    ///     a rehash.
    ///
    /// Both entry points must clear the same way, or `pairs()` yields
    /// `(key, nil)` zombies.
    pub(super) fn clear_existing_slot(&mut self, k: Value) {
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            self.aset(i as usize - 1, Value::Nil);
            return;
        }
        if let Some(idx) = self.find_node(k) {
            self.nodes_mut()[idx].val = Value::Nil;
        }
    }

    /// Integer-keyed variant of [`Self::set`].
    pub fn set_int(&mut self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError> {
        if self.is_readonly() {
            return Err(TableError::ReadOnly);
        }
        self.set_int_raw(heap, i, val)
    }

    /// [`Self::set_int`] for a table the caller knows is not read-only: one
    /// it just made (a vararg or `table.pack` table), or one whose flag it
    /// has tested.
    #[inline]
    pub(crate) fn set_int_raw(
        &mut self,
        heap: &mut Heap,
        i: i64,
        val: Value,
    ) -> Result<(), TableError> {
        self.set_norm(heap, Value::Int(i), val)
    }

    /// `k` is already normalized (no nil, no NaN, integral floats → Int).
    #[inline]
    pub(super) fn set_norm(
        &mut self,
        heap: &mut Heap,
        k: Value,
        v: Value,
    ) -> Result<(), TableError> {
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            // Live array slot + Nil write goes through the shared
            // tombstone routine (see `clear_existing_slot`).
            if v.is_nil() {
                self.clear_existing_slot(k);
            } else {
                self.aset(i as usize - 1, v);
            }
            return Ok(());
        }
        if let Some(idx) = self.find_node(k) {
            if v.is_nil() {
                self.clear_existing_slot(k);
            } else {
                // may revive a tombstone: a metamethod can appear. A
                // read-only table never gets here (every write path tests
                // the mark first), so clearing its mark with the rest of
                // `aux` cannot happen
                debug_assert!(!self.is_readonly());
                self.hdr.aux = 0;
                self.nodes_mut()[idx].val = v;
            }
            return Ok(());
        }
        if v.is_nil() {
            return Ok(()); // absent key set to nil: nothing to record
        }
        self.insert_new(heap, k, v)
    }

    pub(super) fn insert_new(
        &mut self,
        heap: &mut Heap,
        k: Value,
        v: Value,
    ) -> Result<(), TableError> {
        // as in `set_norm`: never a read-only table
        debug_assert!(!self.is_readonly());
        self.hdr.aux = 0;
        if self.nodes().is_empty() {
            self.rehash(heap, k)?;
            return self.set_norm(heap, k, v);
        }
        let mp = self.main_position(k);
        // A truly empty slot (key=Nil, !dead_key) is free for direct placement.
        // A dead-key slot still belongs to some chain (its `next` points to a
        // live entry the chain reaches), so we treat it as occupied here and
        // route the new key through the collision path below — that preserves
        // the back-links into this slot from other nodes' `next` fields.
        if self.nodes()[mp].is_free() {
            self.nodes_mut()[mp] = Node::new(k, v, NONE);
            return Ok(());
        }
        let Some(free) = self.free_pos() else {
            self.rehash(heap, k)?;
            return self.set_norm(heap, k, v);
        };
        // Dead-key slot: it carries no live key, so by definition nobody else
        // counts it as "their main position owner". We give it directly to
        // the new key but preserve `next` so the chain it sits inside still
        // reaches its downstream entries.
        if self.nodes()[mp].dead_key {
            let preserved_next = self.nodes()[mp].next;
            self.nodes_mut()[mp] = Node::new(k, v, preserved_next);
            return Ok(());
        }
        let other_mp = self.main_position(self.nodes()[mp].key());
        if other_mp != mp {
            // colliding node is out of its main position: relocate it to the
            // free slot and take its place
            let mut prev = other_mp;
            while self.nodes()[prev].next != mp as i32 {
                prev = self.nodes()[prev].next as usize;
            }
            self.nodes_mut()[prev].next = free as i32;
            let moved = self.nodes()[mp];
            self.nodes_mut()[free] = moved;
            self.nodes_mut()[mp] = Node::new(k, v, NONE);
        } else {
            // colliding node owns this position: chain the new node behind it
            let next = self.nodes()[mp].next;
            self.nodes_mut()[free] = Node::new(k, v, next);
            self.nodes_mut()[mp].next = free as i32;
        }
        Ok(())
    }

    pub(super) fn free_pos(&mut self) -> Option<usize> {
        while self.lastfree > 0 {
            self.lastfree -= 1;
            let n = &self.nodes()[self.lastfree as usize];
            // Dead-key slots are still occupied for chain purposes (their
            // `next` may be the only path to a downstream entry) — don't
            // hand them out as free.
            if n.is_free() {
                return Some(self.lastfree as usize);
            }
        }
        None
    }
}
