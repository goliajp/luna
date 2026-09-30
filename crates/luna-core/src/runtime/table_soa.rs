//! The SoA + Robin Hood open-addressing hash part of `Table`, kept beside
//! the chain-walk path that tables use today.

use super::*;

// =====================================================================
// SoA + Robin Hood open-addressing hash part.
//
// Parallel to the chain-walk path: the chain `nodes` / `lastfree` is
// the authoritative read path, and nothing outside this block and its
// tests calls into it yet. These methods operate only on the `keys` /
// `vals` / `meta` / `tombstones` SoA arrays — chain state is never
// touched.
//
// Layout invariants the methods below maintain:
//   - `keys.len() == vals.len() == meta.len()`, all power-of-two
//     (or zero in the empty-stub state)
//   - `meta[i] = meta_bits::EMPTY` iff slot i is free
//   - tombstoned slots are scanned past by find but reused by insert
//   - `tombstones` counts the meta slots with TOMBSTONE_BIT set
//   - load factor (live + tombstone) / cap is kept ≤ 0.75 via
//     `soa_grow_if_needed`, which bounds PSL
//   - rehash is REFUSED when `iter_depth > 0` (nothing increments the
//     counter yet, so the refusal path is unreachable today)
// =====================================================================

/// Initial SoA capacity when growing from empty. Power of two.
/// Picked at 4 so a 3-element table doesn't trigger an immediate
/// regrowth.
#[allow(dead_code)] // not yet wired into the public table paths
pub(crate) const SOA_INITIAL_CAP: usize = 4;

/// High load-factor threshold (3/4). SoA grow trigger. PSL_MAX is the u16 14-bit value so
/// long-tail PSL overruns are recoverable via grow-retry.
#[allow(dead_code)]
const SOA_LOAD_NUM: usize = 3;
#[allow(dead_code)]
const SOA_LOAD_DEN: usize = 4;

/// Tombstone density threshold (1/4). When tombstones/cap ≥ 25%
/// the next non-resize-triggering rehash compacts them.
#[allow(dead_code)]
const SOA_TOMB_NUM: usize = 1;
#[allow(dead_code)]
const SOA_TOMB_DEN: usize = 4;

#[allow(dead_code)] // not yet wired into public set/get/next
impl Table {
    /// Current SoA hash-part capacity in slots (0 = empty stub).
    #[inline]
    pub(crate) fn soa_cap(&self) -> usize {
        self.meta.len()
    }

    /// Count of live (occupied & not tombstone) SoA slots.
    /// O(n) — only used by the equivalence tests; the
    /// hot rehash trigger uses `live_estimate = cap*3/4 - tombstones`
    /// implicitly via `soa_grow_if_needed`.
    #[cfg(test)]
    pub(crate) fn soa_live_count(&self) -> usize {
        self.meta.iter().filter(|&&m| meta_bits::is_live(m)).count()
    }

    /// Count of occupied (live OR tombstoned) SoA slots; this is
    /// the value the load factor compares against `cap * 3/4`.
    #[inline]
    fn soa_occupied_count(&self) -> usize {
        // O(n) sweep on each insert, so the worst case is bounded by
        // per-insert amortised cost. A counter maintained incrementally
        // would avoid the sweep if it ever shows up in profiles.
        self.meta
            .iter()
            .filter(|&&m| meta_bits::is_occupied(m))
            .count()
    }

    /// Robin Hood lookup. Returns the slot index of a *live*
    /// matching key, or None if absent. Walks past tombstones (they
    /// preserve probe chains). Returns None if the SoA cap is zero
    /// (empty-stub state). Bound by `cap` probes; in practice
    /// expected ≤ 8 at load 0.75.
    pub(crate) fn soa_find_slot(&self, k: Value) -> Option<usize> {
        let cap = self.meta.len();
        if cap == 0 {
            return None;
        }
        let mask = cap - 1;
        let mut idx = (hash_key(k) as usize) & mask;
        // Walk until empty slot or wrap. The `steps <= cap` bound
        // is a safety net: a properly maintained Robin Hood table
        // with load < 1 always has at least one empty slot, so a
        // full wrap means table invariant violation.
        for _ in 0..cap {
            let m = self.meta[idx];
            if !meta_bits::is_occupied(m) {
                return None;
            }
            if !meta_bits::is_tombstone(m) && self.keys[idx].raw_eq(k) {
                return Some(idx);
            }
            idx = (idx + 1) & mask;
        }
        None
    }

    /// Allocate fresh SoA arrays at `new_cap` (power of two) and
    /// re-insert every live entry from the old SoA arrays. Tombstones
    /// are dropped (count resets to 0). Used by `soa_grow_if_needed`
    /// (new_cap = max(SOA_INITIAL_CAP, 2*cap)) and by tombstone
    /// compaction (new_cap = cap).
    ///
    /// IMPORTANT: rehash MUST NOT fire while `iter_depth > 0`. All
    /// current callers enter from non-iteration paths.
    fn soa_rehash_to(&mut self, heap: &mut Heap, new_cap: usize) -> Result<(), TableError> {
        debug_assert!(new_cap.is_power_of_two() && new_cap > 0);
        let before = self.internal_bytes();
        // Snapshot old live entries. This list is the canonical
        // "must be present after rehash" set; we restart from it on
        // any PSL-overflow retry.
        let mut survivors: Vec<(Value, Value)> = Vec::with_capacity(self.meta.len());
        for i in 0..self.meta.len() {
            if meta_bits::is_live(self.meta[i]) {
                survivors.push((self.keys[i], self.vals[i]));
            }
        }
        // Install fresh empty arrays at `new_cap`. On PSL overflow
        // during the re-insert pass (extremely rare with the 14-bit
        // PSL budget — would need a pathological hash distribution),
        // double the cap and replay the original `survivors` list
        // from scratch. We don't try to salvage partial work — the
        // rare-path retry cost is bounded by O(n × max_doublings),
        // and max_doublings has a hard MAX_ASIZE ceiling.
        let mut cap = new_cap;
        loop {
            if cap > MAX_ASIZE {
                return Err(TableError::Overflow);
            }
            self.keys = vec![Value::Nil; cap].into_boxed_slice();
            self.vals = vec![Value::Nil; cap].into_boxed_slice();
            self.meta = vec![meta_bits::EMPTY; cap].into_boxed_slice();
            self.tombstones = 0;
            let mut overflowed = false;
            for (k, v) in survivors.iter().copied() {
                if self.soa_place_known_absent(k, v).is_err() {
                    overflowed = true;
                    break;
                }
            }
            if !overflowed {
                break;
            }
            cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
        }
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        Ok(())
    }

    /// Raw rob-from-rich placement for a key known to be absent
    /// from the SoA arrays. Used by `soa_rehash_to` (re-insert pass)
    /// and by `soa_insert` (new-key path after the explicit
    /// soa_find_slot check). This routine does NOT auto-grow on a
    /// load-factor trigger (caller's responsibility), but hands the
    /// pending pair back as `Err((k, v))` when the probe sequence
    /// passes `meta_bits::PSL_MAX` before an empty slot turns up. The
    /// caller (`soa_insert`) grows and retries.
    ///
    /// On success returns the slot index where the new key landed
    /// (after any rob-from-rich shuffle, the original `k` value is at
    /// this returned index).
    fn soa_place_known_absent(&mut self, k: Value, v: Value) -> Result<usize, (Value, Value)> {
        let cap = self.meta.len();
        debug_assert!(cap > 0);
        let mask = cap - 1;
        let landing = (hash_key(k) as usize) & mask;
        let mut idx = landing;
        let mut cur_psl: u16 = 0;
        let mut cur_key = k;
        let mut cur_val = v;
        let mut placed_at: Option<usize> = None;
        for _ in 0..cap {
            let m = self.meta[idx];
            if !meta_bits::is_occupied(m) || meta_bits::is_tombstone(m) {
                if meta_bits::is_tombstone(m) {
                    self.tombstones = self.tombstones.saturating_sub(1);
                }
                self.meta[idx] = meta_bits::pack(cur_psl, false);
                self.keys[idx] = cur_key;
                self.vals[idx] = cur_val;
                return Ok(placed_at.unwrap_or(idx));
            }
            let stored_psl = meta_bits::psl(m);
            if cur_psl > stored_psl {
                // Rob: swap cur into this slot, evict stored to continue.
                std::mem::swap(&mut cur_key, &mut self.keys[idx]);
                std::mem::swap(&mut cur_val, &mut self.vals[idx]);
                self.meta[idx] = meta_bits::pack(cur_psl, false);
                if placed_at.is_none() {
                    placed_at = Some(idx);
                }
                cur_psl = stored_psl;
            }
            idx = (idx + 1) & mask;
            cur_psl = cur_psl.saturating_add(1);
            if cur_psl > meta_bits::PSL_MAX {
                // PSL exceeds the 14-bit storage budget — exceptionally
                // rare with 16384 max. Caller (soa_insert / rehash
                // outer loop) handles by growing & retrying. Partial
                // state: all entries are still in the table EXCEPT
                // `(cur_key, cur_val)` which is the latest homeless
                // evictee — return it so caller can re-issue.
                return Err((cur_key, cur_val));
            }
        }
        // Wrapped cap probes with no free slot — invariant violation
        // (load < 1 should guarantee at least one empty). Signal as
        // PSL-overflow equivalent so caller grows + retries.
        Err((cur_key, cur_val))
    }

    /// Grow SoA capacity if the load factor is at or above the
    /// 0.75 trigger. Doubles cap; from empty grows to SOA_INITIAL_CAP.
    fn soa_grow_if_needed(&mut self, heap: &mut Heap) -> Result<(), TableError> {
        // defer rehash when an iterator is in flight (iter_depth is
        // never incremented yet, so this does not fire today)
        if self.iter_depth > 0 {
            return Ok(());
        }
        let cap = self.meta.len();
        if cap == 0 {
            return self.soa_rehash_to(heap, SOA_INITIAL_CAP);
        }
        let occupied = self.soa_occupied_count();
        if occupied * SOA_LOAD_DEN >= cap * SOA_LOAD_NUM {
            let new_cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
            return self.soa_rehash_to(heap, new_cap);
        }
        // Tombstone compaction (same cap, drops tombstones).
        if self.tombstones as usize * SOA_TOMB_DEN >= cap * SOA_TOMB_NUM {
            return self.soa_rehash_to(heap, cap);
        }
        Ok(())
    }

    /// Insert (or update) `(k, v)` in the SoA hash part. Routes
    /// through `soa_find_slot` first so an existing key updates its
    /// val in place; otherwise rob-from-rich places a new entry.
    /// Auto-rehashes if the load factor would exceed 0.75 OR if the
    /// place chain runs into a PSL overflow on a pathological hash
    /// distribution.
    ///
    /// Only the equivalence tests call this; it is not yet hooked
    /// into public `set` / `set_norm`.
    pub(crate) fn soa_insert(
        &mut self,
        heap: &mut Heap,
        k: Value,
        v: Value,
    ) -> Result<(), TableError> {
        debug_assert!(!matches!(k, Value::Nil));
        // 1. Update-in-place if key is already present (live slot).
        if let Some(idx) = self.soa_find_slot(k) {
            self.vals[idx] = v;
            return Ok(());
        }
        // 2. New key: ensure capacity, then place. On PSL-overflow
        // from the place chain (extremely rare with 14-bit PSL budget),
        // grow + rehash with the homeless evictee merged in.
        // `soa_rehash_with_extra` handles further retries internally,
        // bounded by MAX_ASIZE.
        self.soa_grow_if_needed(heap)?;
        match self.soa_place_known_absent(k, v) {
            Ok(_) => Ok(()),
            Err(homeless) => {
                let cap = self.meta.len();
                let new_cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
                self.soa_rehash_with_extra(heap, new_cap, homeless)
            }
        }
    }

    /// Rehash to `new_cap` while merging in an extra (k, v) pair
    /// not currently in the SoA arrays. Used by `soa_insert` to
    /// recover from PSL overflow: the homeless evictee from the failed
    /// place chain gets appended to the survivor list before the
    /// re-insert pass.
    fn soa_rehash_with_extra(
        &mut self,
        heap: &mut Heap,
        new_cap: usize,
        extra: (Value, Value),
    ) -> Result<(), TableError> {
        let before = self.internal_bytes();
        let mut survivors: Vec<(Value, Value)> = Vec::with_capacity(self.meta.len() + 1);
        for i in 0..self.meta.len() {
            if meta_bits::is_live(self.meta[i]) {
                survivors.push((self.keys[i], self.vals[i]));
            }
        }
        // Avoid duplicating the extra if its key was already placed at
        // some slot during the failed rob chain (the rob may have
        // landed the original input into a slot before overflowing on
        // a downstream evictee — that case the meta-walk above picks
        // it up).
        if !survivors.iter().any(|(k, _)| k.raw_eq(extra.0)) {
            survivors.push(extra);
        }
        let mut cap = new_cap;
        loop {
            if cap > MAX_ASIZE {
                return Err(TableError::Overflow);
            }
            self.keys = vec![Value::Nil; cap].into_boxed_slice();
            self.vals = vec![Value::Nil; cap].into_boxed_slice();
            self.meta = vec![meta_bits::EMPTY; cap].into_boxed_slice();
            self.tombstones = 0;
            let mut overflowed = false;
            for (k, v) in survivors.iter().copied() {
                if self.soa_place_known_absent(k, v).is_err() {
                    overflowed = true;
                    break;
                }
            }
            if !overflowed {
                break;
            }
            cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
        }
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        Ok(())
    }

    /// Read SoA hash part. Mirrors `get_hash` but reads from
    /// keys/vals/meta rather than nodes. Used by the equivalence
    /// tests; not yet hooked into public `get` / `get_hash`.
    pub(crate) fn soa_get(&self, k: Value) -> Value {
        match self.soa_find_slot(k) {
            Some(idx) => self.vals[idx],
            None => Value::Nil,
        }
    }

    /// Tombstone deletion. Marks the live slot for `k` as
    /// tombstoned, preserving the slot index (no backward shift).
    /// Slot-index stability is the PUC `next()` iteration invariant
    /// — `nextvar.lua:520-521` requires that deleting prior keys
    /// during a `pairs` traversal does NOT move unvisited keys.
    /// Backward-shift deletion would violate this; tombstones are
    /// the standard Robin Hood resolution.
    ///
    /// keys[idx] / vals[idx] are reset to Nil so the GC marker is
    /// not held to the previous entries — only the tombstone bit
    /// distinguishes "occupied tombstone" from "free empty".
    ///
    /// Returns true if the key was found and deleted, false if absent.
    ///
    /// Not yet hooked into public `set(k, Nil)`; that has to move
    /// together with `next()`.
    pub(crate) fn soa_delete(&mut self, k: Value) -> bool {
        if let Some(idx) = self.soa_find_slot(k) {
            let psl = meta_bits::psl(self.meta[idx]);
            self.meta[idx] = meta_bits::pack(psl, true);
            self.keys[idx] = Value::Nil;
            self.vals[idx] = Value::Nil;
            self.tombstones = self.tombstones.saturating_add(1);
            true
        } else {
            false
        }
    }
}
