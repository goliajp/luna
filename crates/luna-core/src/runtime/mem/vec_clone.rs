//! `LVec` operations that copy elements: clones appended, filled, or
//! taken from the vector's own front.

use super::ctx::{MemRef, Oom};
use super::vec::LVec;

impl<T: Clone> LVec<T> {
    /// Append clones of `s`.
    #[inline]
    pub fn extend_from_slice(&mut self, s: &[T]) -> Result<(), Oom> {
        self.reserve(s.len())?;
        for v in s {
            // SAFETY: room for all of `s` was reserved above
            unsafe { self.ptr.as_ptr().add(self.len).write(v.clone()) };
            self.len += 1;
        }
        Ok(())
    }

    /// Grow with clones of `v` or shrink to length `n`.
    #[inline]
    pub fn resize(&mut self, n: usize, v: T) -> Result<(), Oom> {
        if n <= self.len {
            self.truncate(n);
            return Ok(());
        }
        self.reserve(n - self.len)?;
        self.fill_to(n, v);
        Ok(())
    }

    /// Append clones of `v` up to length `n`.
    #[inline(always)]
    pub(super) fn fill_to(&mut self, n: usize, v: T) {
        debug_assert!(n <= self.cap);
        let mut len = self.len;
        while len < n {
            // SAFETY: the callers reserved room for `n` elements
            unsafe { self.ptr.as_ptr().add(len).write(v.clone()) };
            len += 1;
        }
        self.len = len;
    }

    /// A copy on the same context.
    pub fn try_clone(&self) -> Result<LVec<T>, Oom> {
        let mut v = LVec::with_capacity(self.mem(), self.len)?;
        v.extend_from_slice(self)?;
        Ok(v)
    }

    /// A vector on `mem` holding clones of `s`.
    #[inline]
    pub fn from_slice(mem: MemRef, s: &[T]) -> Result<LVec<T>, Oom> {
        let mut v = LVec::with_capacity(mem, s.len())?;
        v.extend_from_slice(s)?;
        Ok(v)
    }
}

impl<T: Copy> LVec<T> {
    /// Append a copy of the first `k` elements, which must be there,
    /// without first filling the room it goes into.
    #[inline]
    pub fn extend_from_front(&mut self, k: usize) -> Result<(), Oom> {
        assert!(k <= self.len);
        self.reserve(k)?;
        // SAFETY: room for `k` more was reserved above, so the slots
        // `len..len + k` are in the block and unused; the source `..k` is
        // initialized and lies wholly below them
        unsafe {
            std::ptr::copy_nonoverlapping(self.ptr.as_ptr(), self.ptr.as_ptr().add(self.len), k)
        };
        self.len += k;
        Ok(())
    }
}
