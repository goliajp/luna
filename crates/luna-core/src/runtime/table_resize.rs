//! Rehash sizing and resizing of the array and hash parts.

use super::*;

impl Table {
    pub(super) fn rehash(&mut self, heap: &mut Heap, pending: Value) -> Result<(), TableError> {
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
        let before = self.internal_bytes();
        // snapshot the old array entries before we
        // re-install the backing. The active buffer can be inline OR
        // slab; `array_ptr` already points to whichever it is, so
        // walking via raw offsets works the same for either case.
        let old_asize = self.asize as usize;
        let old_array = self.array_ptr;
        // growing keeps every array entry at its index, so the old backing
        // is copied as is (PUC `luaH_resize` reallocates in place);
        // shrinking re-inserts entry by entry below
        let grow = new_asize >= old_asize && old_asize > 0;
        let mut old_pairs: Vec<(u8, RawVal)> = Vec::with_capacity(if grow { 0 } else { old_asize });
        let mut old_slab: *mut u8 = std::ptr::null_mut();
        let mut old_inline = [0u64; INLINE_U64S];
        if grow {
            if old_asize as u64 <= INLINE_ASIZE {
                // SAFETY: exclusive &mut self; the inline bytes are read through the cell
                old_inline = unsafe { *self.inline_storage.get() };
            } else {
                old_slab = self.array_ptr;
            }
        } else if old_asize > 0 {
            // SAFETY: `array_ptr` was set up by `Heap::new_table` or
            // an earlier `resize`; it covers `old_asize * 9` bytes
            // (avals + atags).
            let avals_base = self.array_base() as *const RawVal;
            let atags_base = unsafe { self.array_base().add(old_asize * 8) as *const u8 };
            for i in 0..old_asize {
                // SAFETY: `i < array_len` is enforced by the surrounding loop bound; `atags_base` / `avals_base` point into the table's parallel arrays allocated in lockstep by `init_array_ptr`.
                let tag = unsafe { *atags_base.add(i) };
                // SAFETY: `i < array_len` is enforced by the surrounding loop bound; `atags_base` / `avals_base` point into the table's parallel arrays allocated in lockstep by `init_array_ptr`.
                let val = unsafe { *avals_base.add(i) };
                old_pairs.push((tag, val));
            }
        }
        let old_nodes = self.take_hash_part();

        // Install the new array backing first, then update `array_ptr`
        // (before potentially dropping the old slab via the assignment
        // below) so the JIT never observes a stale pointer.
        self.asize = new_asize as u64;
        if new_asize <= INLINE_ASIZE as usize {
            // Inline path — zero the inline buffer; drop any prior
            // external slab.
            // SAFETY: exclusive &mut self; write through the cell to
            // stay on the raw-pointer access path (no &mut borrow of
            // the array contents is ever formed).
            unsafe {
                *self.inline_storage.get() = [0; INLINE_U64S];
            }
            self.array_ptr = self.inline_storage.get() as *mut u8;
        } else {
            self.array_ptr = Self::alloc_slab(new_asize);
        }
        if !grow && old_asize as u64 > INLINE_ASIZE {
            // shrinking or rebuilding: the old entries were copied out above
            // SAFETY: the old array part lived in a slab of `old_asize`
            unsafe { Self::free_slab(old_array, old_asize) };
        }

        let hsize = if hash_entries == 0 {
            0
        } else {
            hash_entries.next_power_of_two()
        };
        self.set_hash_part(vec![Node::EMPTY; hsize].into_boxed_slice());
        self.lastfree = hsize as u32;
        // PUC `g->GCtotalbytes` analogue: credit (or debit) the box-size
        // delta so `Heap.bytes` reflects this table's actual internal
        // memory. `free_obj` subtracts `internal_bytes()` on the way out.
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        if grow {
            let src: *const u8 = if old_asize as u64 <= INLINE_ASIZE {
                old_inline.as_ptr() as *const u8
            } else {
                old_slab as *const u8
            };
            // SAFETY: both backings use the `[avals: n×8][atags: n]` layout;
            // the new one holds `new_asize >= old_asize` zero (nil) slots
            unsafe {
                let dst = self.array_base();
                std::ptr::copy_nonoverlapping(src, dst, old_asize * 8);
                std::ptr::copy_nonoverlapping(
                    src.add(old_asize * 8),
                    dst.add(new_asize * 8),
                    old_asize,
                );
            }
            if !old_slab.is_null() {
                // SAFETY: the old array part lived in a slab of `old_asize`
                unsafe { Self::free_slab(old_slab, old_asize) };
            }
            // growing appends nil slots, so the count stays; the prefix
            // may lag behind the run (a refill scans only 64 slots ahead,
            // a method-JIT store extends it by one) and catches up here
            let atags = self.atags();
            let mut p = self.aprefix as usize;
            while p < old_asize && atags[p] != raw::NIL {
                p += 1;
            }
            self.aprefix = p as u32;
            #[cfg(debug_assertions)]
            {
                let kept = (self.acount, self.aprefix);
                self.recount_array();
                debug_assert_eq!(kept, (self.acount, self.aprefix));
            }
        } else {
            self.recount_array();
        }
        // Re-insert old array entries via the public set_norm path
        // (which handles rehashing if the new array shrinks below the
        // entry count).
        for (i, (tag, val)) in old_pairs.into_iter().enumerate() {
            if tag != raw::NIL {
                // SAFETY: `tag` and the raw value come from this table's parallel `atags` / `avals` arrays, which the table writers always keep in sync — the tag byte matches the raw payload's discriminator (see `runtime::value` `raw` module).
                let v = unsafe { Value::pack(tag, val) };
                let _ = self.set_norm(heap, Value::Int(i as i64 + 1), v);
            }
        }
        for n in old_nodes.iter() {
            if !n.val.is_nil() {
                let _ = self.set_norm(heap, n.key(), n.val);
                if n.neg_zero {
                    self.mark_neg_zero();
                }
            }
        }
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
