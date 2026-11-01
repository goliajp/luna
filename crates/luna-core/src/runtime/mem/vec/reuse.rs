//! Handing a vector's block on: to another element type, to a boxed
//! slice, or back to the context.

use super::LVec;
use std::ptr::NonNull;

impl<T> LVec<T> {
    /// This vector emptied, as a vector of another element type with the
    /// same layout keeping the block (so a vector of borrowed records can
    /// be kept across borrows of different lifetimes); a vector of another
    /// layout gives its block back.
    pub fn recycle<U>(mut self) -> LVec<U> {
        self.clear();
        let same = std::mem::size_of::<T>() == std::mem::size_of::<U>()
            && std::mem::align_of::<T>() == std::mem::align_of::<U>()
            && !Self::ZST;
        let mut v = LVec::new(self.mem);
        if same && self.cap != 0 {
            let me = std::mem::ManuallyDrop::new(self);
            v.ptr = me.ptr.cast();
            v.cap = me.cap;
        }
        v
    }

    /// The elements as a boxed slice, giving back the spare capacity.
    #[inline]
    pub fn into_slice(mut self) -> super::super::LSlice<T> {
        self.shrink_to_fit();
        let me = std::mem::ManuallyDrop::new(self);
        // SAFETY: the block holds exactly `len` initialized elements (or is
        // dangling with none), allocated by `mem`; ownership moves over
        unsafe { super::super::LSlice::from_raw_parts(me.ptr, me.len, me.mem) }
    }

    /// Give back the spare capacity; when the context cannot shrink the
    /// block, it stays as it is.
    pub fn shrink_to_fit(&mut self) {
        if Self::ZST || self.cap == self.len {
            return;
        }
        let old = Self::layout(self.cap).expect("the current block has a layout");
        let ctx = self.mem.ctx();
        if self.len == 0 {
            // SAFETY: `ptr` is this vector's live block of `old`
            unsafe { ctx.free(self.ptr.cast(), old) };
            self.ptr = NonNull::dangling();
            self.cap = 0;
            return;
        }
        let new = Self::layout(self.len).expect("smaller than the current block");
        // SAFETY: `ptr` is this vector's live block of `old`; the new size is
        // not 0
        if let Some(p) = unsafe { ctx.realloc(self.ptr.cast(), old, new.size()) } {
            self.ptr = p.cast();
            self.cap = self.len;
        }
    }
}
