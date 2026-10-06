//! `LVec` growth with no memory error to return: inside a load it unwinds
//! to the load's entry, elsewhere it ends the process (see
//! `oom_abort`).

use std::alloc::Layout;

use super::ctx::MemRef;
use super::vec::LVec;

/// Growth that ends the process when the allocation fails, as the
/// standard library's `Vec` does. For the places that cannot report a
/// memory error yet.
impl<T> LVec<T> {
    /// [`LVec::with_capacity`], ending the process on failure.
    #[inline]
    pub fn with_capacity_or_abort(mem: MemRef, n: usize) -> LVec<T> {
        let mut v = LVec::new(mem);
        v.reserve_exact_or_abort(n);
        v
    }

    /// [`LVec::reserve_exact`], ending the process on failure.
    #[inline]
    pub fn reserve_exact_or_abort(&mut self, extra: usize) {
        if self.reserve_exact(extra).is_err() {
            vec_oom::<T>(self.len.saturating_add(extra))
        }
    }

    /// [`LVec::push`], ending the process on failure.
    #[inline(always)]
    pub fn push_or_abort(&mut self, v: T) {
        // the length is read once: the write below goes through a raw
        // pointer, after which it would have to be loaded again
        let len = self.len;
        if len == self.cap {
            self.reserve_slow_or_abort(1);
        }
        // SAFETY: `len < cap`, so the slot is inside the block and unused
        unsafe { self.ptr.as_ptr().add(len).write(v) };
        self.len = len + 1;
    }

    /// [`LVec::reserve_slow`], ending the process on failure.
    #[inline(never)]
    #[cold]
    fn reserve_slow_or_abort(&mut self, extra: usize) {
        if self.reserve_slow(extra).is_err() {
            vec_oom::<T>(self.len.saturating_add(extra))
        }
    }

    /// [`LVec::insert`], ending the process on failure.
    pub fn insert_or_abort(&mut self, i: usize, v: T) {
        if self.insert(i, v).is_err() {
            vec_oom::<T>(self.len + 1)
        }
    }

    /// [`LVec::reserve`], ending the process on failure.
    #[inline(always)]
    pub fn reserve_or_abort(&mut self, extra: usize) {
        if self.cap - self.len < extra {
            self.reserve_slow_or_abort(extra);
        }
    }
}

impl<T: Clone> LVec<T> {
    /// [`LVec::resize`], ending the process on failure.
    #[inline]
    pub fn resize_or_abort(&mut self, n: usize, v: T) {
        if n <= self.len {
            self.truncate(n);
            return;
        }
        self.reserve_or_abort(n - self.len);
        self.fill_to(n, v);
    }

    /// [`LVec::from_slice`], ending the process on failure.
    pub fn from_slice_or_abort(mem: MemRef, s: &[T]) -> LVec<T> {
        LVec::from_slice(mem, s).unwrap_or_else(|_| vec_oom::<T>(s.len()))
    }

    /// [`LVec::extend_from_slice`], ending the process on failure.
    pub fn extend_from_slice_or_abort(&mut self, s: &[T]) {
        if self.extend_from_slice(s).is_err() {
            vec_oom::<T>(self.len.saturating_add(s.len()))
        }
    }
}

#[cold]
#[inline(never)]
fn vec_oom<T>(n: usize) -> ! {
    super::oom_abort(Layout::array::<T>(n).unwrap_or(Layout::new::<T>()))
}
