//! Appending past a full array part: the common way a table built by
//! `t[#t + 1] = v` grows.

use super::*;

#[cfg(test)]
thread_local! {
    /// Tests turn the shortcut off to compare it with the full rehash.
    pub(super) static FULL_REHASH_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// How many rehashes the shortcut took.
    pub(super) static APPEND_REHASHES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl Table {
    /// `rehash` for the new key `asize + 1` of a table whose array part is
    /// full and which has no hash part, when it takes no counting: the
    /// integer keys are exactly `1..=asize + 1`, so the array part becomes
    /// the next power of two that holds them and nothing goes to a hash
    /// part. `None` when the table is not in that state.
    pub(super) fn rehash_append(
        &mut self,
        heap: &mut Heap,
        pending: Value,
    ) -> Option<Result<(), TableError>> {
        #[cfg(test)]
        if FULL_REHASH_ONLY.with(|c| c.get()) {
            return None;
        }
        let asize = self.asize;
        let Value::Int(k) = pending else { return None };
        if k as u64 != asize + 1 || u64::from(self.acount) != asize || self.node_mask != u32::MAX {
            return None;
        }
        let new_asize = (asize as usize + 1).next_power_of_two();
        if new_asize > MAX_ASIZE {
            return Some(Err(TableError::Overflow));
        }
        #[cfg(test)]
        APPEND_REHASHES.with(|c| c.set(c.get() + 1));
        if asize > INLINE_ASIZE {
            self.grow_slab(heap, new_asize);
        } else {
            self.resize(heap, new_asize, 0);
        }
        Some(Ok(()))
    }

    /// Grow a slab-backed array part, its slots all non-nil and kept where
    /// they are, to `new_asize` with nil slots after them: the slab is
    /// reallocated (in place when the allocator can) and the tags move up
    /// to their new offset.
    fn grow_slab(&mut self, heap: &mut Heap, new_asize: usize) {
        let before = self.internal_bytes();
        let old = self.asize as usize;
        debug_assert!(old as u64 > INLINE_ASIZE && new_asize > old);
        let old_layout = Self::slab_layout(old);
        let new_layout = Self::slab_layout(new_asize);
        // SAFETY: `array_ptr` is the slab `alloc_slab(mem, old)` (or an
        // earlier `grow_slab`) made with `old_layout` from the heap's
        // context; the new size is non-zero
        let p = match unsafe {
            heap.mem_ctx().realloc(
                std::ptr::NonNull::new_unchecked(self.array_ptr),
                old_layout,
                new_layout.size(),
            )
        } {
            Some(p) => p.as_ptr(),
            None => crate::runtime::mem::oom_abort(new_layout),
        };
        // SAFETY: the block holds `new_asize * 9` bytes and more; the old
        // tags sit at `old * 8`, the new ones go to `new_asize * 8` (the
        // ranges may overlap, hence `copy`), then the new value slots and
        // the new tags are cleared to nil
        unsafe {
            std::ptr::copy(p.add(old * 8), p.add(new_asize * 8), old);
            std::ptr::write_bytes(p.add(old * 8), 0, (new_asize - old) * 8);
            std::ptr::write_bytes(p.add(new_asize * 8 + old), 0, new_asize - old);
        }
        self.array_ptr = p;
        self.asize = new_asize as u64;
        // every old slot holds a value: the run of them is the whole old
        // part, as the full resize's catch-up scan finds
        self.aprefix = old as u32;
        heap.apply_bytes_delta(before, self.internal_bytes());
    }
}
