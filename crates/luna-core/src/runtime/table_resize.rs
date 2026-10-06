//! Rehash sizing and resizing of the array and hash parts.

use super::*;

impl Table {
    pub(super) fn rehash(&mut self, heap: &mut Heap, pending: Value) -> Result<(), TableError> {
        if let Some(r) = self.rehash_append(heap, pending) {
            return r;
        }
        if self.dialect() == Dialect::L55 {
            return self.rehash_55(heap, pending);
        }
        let mut nums = [0usize; 65];
        let mut int_keys = 0usize;
        let mut total = 1; // the pending key
        if let Value::Int(i) = pending
            && i >= 1
        {
            nums[ceil_log2(i as u64)] += 1;
            int_keys += 1;
        }
        let asize = self.asize();
        if self.acount as usize == asize {
            // a full array part: slots 1..=asize all count, bucket by bucket
            // (bucket b holds (2^(b-1), 2^b], bucket 0 holds 1)
            debug_assert!(self.atags().iter().all(|&t| t != raw::NIL));
            let mut lo = 1usize;
            let mut b = 0usize;
            while lo <= asize {
                let hi = (1usize << b).min(asize);
                nums[b] += hi + 1 - lo;
                lo = hi + 1;
                b += 1;
            }
            int_keys += asize;
            total += asize;
        } else {
            let atags = self.atags();
            for (i, &tag) in atags.iter().enumerate() {
                if tag != raw::NIL {
                    nums[ceil_log2(i as u64 + 1)] += 1;
                    int_keys += 1;
                    total += 1;
                }
            }
        }
        for n in self.nodes().iter() {
            if !n.val.is_nil() {
                total += 1;
                if let Value::Int(i) = n.key()
                    && i >= 1
                {
                    nums[ceil_log2(i as u64)] += 1;
                    int_keys += 1;
                }
            }
        }
        // computesizes: optimal array size = largest 2^i with more than 2^(i-1)
        // integer keys in [1, 2^i]
        let mut new_asize = 0usize;
        let mut in_array = 0usize;
        let mut a = 0usize;
        let mut two_to_i = 1usize;
        let mut i = 0usize;
        while int_keys > two_to_i / 2 {
            a += nums[i];
            if a > two_to_i / 2 {
                new_asize = two_to_i;
                in_array = a;
            }
            i += 1;
            match two_to_i.checked_mul(2) {
                Some(n) => two_to_i = n,
                None => break,
            }
        }
        // PUC `luaH_resizearray` raises "table overflow" when the array part
        // would have to grow past MAXASIZE. luna mirrors with `MAX_ASIZE`,
        // checked on both the array and the hash bucket count (the latter is
        // a power-of-two of total - in_array entries).
        if new_asize > MAX_ASIZE {
            return Err(TableError::Overflow);
        }
        let hash_entries = total - in_array;
        if hash_entries > MAX_ASIZE {
            return Err(TableError::Overflow);
        }
        self.resize(heap, new_asize, hash_entries);
        Ok(())
    }

    /// Resize the table's array and hash parts. The array part grows
    /// (or shrinks) to `new_asize` NIL-initialized slots; the hash
    /// part rounds to the next power of two ≥ `hash_entries`. Any
    /// existing entries are re-inserted into the new layout. The
    /// Box growth is debited/credited to `heap.bytes` so `free_obj`
    /// can subtract the symmetric amount.
    ///
    /// `Heap::new_table_sized` calls this on a freshly
    /// adopted empty table to pre-allocate the array part, sparing
    /// the table-fill loop from O(log N) intermediate `rehash`es.
    pub(crate) fn resize(&mut self, heap: &mut Heap, new_asize: usize, hash_entries: usize) {
        let mem = heap.mem();
        let before = self.internal_bytes();
        let hsize = if hash_entries == 0 {
            0
        } else {
            hash_entries.next_power_of_two()
        };
        // both new parts are allocated before the table changes, so a
        // failed allocation leaves it as it was
        let new_slab = if new_asize > INLINE_ASIZE as usize {
            Self::alloc_slab(mem, new_asize)
        } else {
            std::ptr::null_mut()
        };
        let new_nodes = Self::alloc_nodes(mem, hsize);
        // the old array part: its slab stays allocated until its entries
        // are moved over; inline entries are copied out first, since the
        // inline storage may become the new backing
        let old_asize = self.asize as usize;
        let mut old_inline = [0u64; INLINE_U64S];
        let old_slab = if old_asize as u64 > INLINE_ASIZE {
            self.array_ptr
        } else {
            // SAFETY: exclusive &mut self; the inline bytes are read through the cell
            old_inline = unsafe { *self.inline_storage.get() };
            std::ptr::null_mut()
        };
        let old_src: *const u8 = if old_slab.is_null() {
            old_inline.as_ptr() as *const u8
        } else {
            old_slab
        };
        // growing keeps every array entry at its index, so the old backing
        // is copied as is (PUC `luaH_resize` reallocates in place);
        // shrinking re-inserts entry by entry below
        let grow = new_asize >= old_asize && old_asize > 0;
        let (old_nodes, old_nodes_len) = self.take_hash_part();

        // Install the new array backing before anything reads it, so the JIT
        // never observes a stale pointer.
        self.asize = new_asize as u64;
        self.reset_hints();
        if new_slab.is_null() {
            // SAFETY: exclusive &mut self; write through the cell to
            // stay on the raw-pointer access path (no &mut borrow of
            // the array contents is ever formed).
            unsafe {
                *self.inline_storage.get() = [0; INLINE_U64S];
            }
            self.array_ptr = self.inline_storage.get() as *mut u8;
        } else {
            self.array_ptr = new_slab;
        }
        self.set_hash_part(new_nodes, hsize);
        self.lastfree = hsize as u32;
        // PUC `g->GCtotalbytes` analogue: credit (or debit) the box-size
        // delta so `Heap.bytes` reflects this table's actual internal
        // memory. `free_obj` subtracts `internal_bytes()` on the way out.
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        if grow {
            // SAFETY: both backings use the `[avals: n×8][atags: n]` layout;
            // the new one holds `new_asize >= old_asize` zero (nil) slots
            unsafe {
                let dst = self.array_base();
                std::ptr::copy_nonoverlapping(old_src, dst, old_asize * 8);
                std::ptr::copy_nonoverlapping(
                    old_src.add(old_asize * 8),
                    dst.add(new_asize * 8),
                    old_asize,
                );
            }
            // growing appends nil slots, so the count stays; the prefix
            // may lag behind the run (a refill scans only 64 slots ahead,
            // a method-JIT store extends it by one) and catches up here
            if self.aprefix == APREFIX_UNKNOWN {
                self.recount_array();
            } else {
                let atags = self.atags();
                let mut p = self.aprefix as usize;
                while p < old_asize && atags[p] != raw::NIL {
                    p += 1;
                }
                self.aprefix = p as u32;
            }
            #[cfg(debug_assertions)]
            {
                let kept = (self.acount, self.aprefix);
                self.recount_array();
                debug_assert_eq!(kept, (self.acount, self.aprefix));
            }
        } else {
            self.recount_array();
            // Re-insert old array entries via the set_norm path; the new
            // parts were sized to hold them, so this allocates nothing
            let avals = old_src as *const RawVal;
            for i in 0..old_asize {
                // SAFETY: `i < old_asize`, inside the old backing (the inline
                // copy or the old slab, still allocated), whose tag and value
                // arrays the table writers keep in step, so the tag names the
                // value's type
                let v = unsafe {
                    let tag = *old_src.add(old_asize * 8 + i);
                    if tag == raw::NIL {
                        continue;
                    }
                    Value::pack(tag, *avals.add(i))
                };
                let _ = self.set_norm(heap, Value::Int(i as i64 + 1), v);
            }
        }
        if !old_slab.is_null() {
            // SAFETY: the old array part lived in a slab of `old_asize` from
            // this context, and its entries have been moved over
            unsafe { Self::free_slab(mem, old_slab, old_asize) };
        }
        // 5.1–5.3 put the old nodes back last to first, 5.4 / 5.5 first to
        // last; the order decides which keys collide, and so when the
        // table next rehashes
        let backwards = self.dialect() <= Dialect::L53;
        for j in 0..old_nodes_len {
            let i = if backwards { old_nodes_len - 1 - j } else { j };
            // SAFETY: `i` is inside the old hash part, still allocated
            let n = unsafe { *old_nodes.add(i) };
            if !n.val.is_nil() {
                let _ = self.set_norm(heap, n.key(), n.val);
                if n.neg_zero {
                    self.mark_neg_zero();
                }
            }
        }
        // SAFETY: the old hash part came from `alloc_nodes(mem, len)` and its
        // entries have been moved over
        unsafe { Self::free_nodes(mem, old_nodes, old_nodes_len) };
    }

    /// Preallocate the array part (table.create); existing contents are
    /// preserved.
    pub fn ensure_array(&mut self, heap: &mut Heap, n: usize) {
        if n > self.asize() {
            let hash_entries = self.nodes().iter().filter(|nd| !nd.val.is_nil()).count();
            self.resize(heap, n, hash_entries);
        }
    }

    /// Preallocate hash-part capacity (table.create's second size).
    pub fn ensure_hash(&mut self, heap: &mut Heap, n: usize) {
        let entries = self.nodes().iter().filter(|nd| !nd.val.is_nil()).count();
        if n > self.nodes().len() {
            self.resize(heap, self.asize(), n.max(entries));
        }
    }
}
