//! Moving a vector's elements out.

use super::LVec;
use std::marker::PhantomData;
use std::ptr::NonNull;

impl<T> LVec<T> {
    /// Move the elements out into a new vector on the same context,
    /// leaving this one empty with no block.
    pub fn take(&mut self) -> LVec<T> {
        std::mem::replace(self, LVec::new(self.mem))
    }

    /// Move every element out, in order, leaving the vector empty with its
    /// block kept; elements the iterator does not reach are dropped with it.
    pub fn drain_all(&mut self) -> Drain<'_, T> {
        let len = self.len;
        // the elements now belong to the iterator
        self.len = 0;
        Drain {
            ptr: self.ptr,
            next: 0,
            end: len,
            _vec: PhantomData,
        }
    }
}

/// The elements of an [`LVec`] being moved out ([`LVec::drain_all`]).
pub struct Drain<'v, T> {
    pub(super) ptr: NonNull<T>,
    pub(super) next: usize,
    pub(super) end: usize,
    pub(super) _vec: PhantomData<&'v mut LVec<T>>,
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.next == self.end {
            return None;
        }
        // SAFETY: slots `next..end` hold elements the vector gave up (its
        // length is 0) and this iterator reads each once
        let v = unsafe { self.ptr.as_ptr().add(self.next).read() };
        self.next += 1;
        Some(v)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end - self.next;
        (n, Some(n))
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        for _ in self.by_ref() {}
    }
}
