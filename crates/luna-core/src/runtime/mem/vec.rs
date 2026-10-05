//! `LVec<T>`: a growable array whose block comes from a [`MemCtx`].

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

use super::ctx::{BlockKind, MemRef, Oom};

/// A `Vec<T>` whose memory comes from a Vm's allocation context. Growing
/// can fail ([`Oom`]); a failed growth leaves the vector as it was.
pub struct LVec<T> {
    ptr: NonNull<T>,
    cap: usize,
    len: usize,
    mem: MemRef,
    _own: PhantomData<T>,
}

impl<T> LVec<T> {
    const ALIGN_OK: () = assert!(std::mem::align_of::<T>() <= 8);
    const ZST: bool = std::mem::size_of::<T>() == 0;

    /// An empty vector; allocates nothing.
    #[inline]
    pub fn new(mem: MemRef) -> LVec<T> {
        let () = Self::ALIGN_OK;
        LVec {
            ptr: NonNull::dangling(),
            cap: if Self::ZST { usize::MAX } else { 0 },
            len: 0,
            mem,
            _own: PhantomData,
        }
    }

    /// An empty vector with room for `n`.
    #[inline]
    pub fn with_capacity(mem: MemRef, n: usize) -> Result<LVec<T>, Oom> {
        let mut v = LVec::new(mem);
        v.reserve_exact(n)?;
        Ok(v)
    }

    /// The handle the vector allocates through.
    #[inline(always)]
    pub fn mem(&self) -> MemRef {
        self.mem
    }

    /// Number of elements.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no elements.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Elements the block holds without growing.
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Pointer to the first element.
    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    /// Mutable pointer to the first element.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Take over a block of exactly `len` initialized elements; the
    /// vector is empty and has no block.
    ///
    /// # Safety
    /// `ptr` is a block of `len` elements allocated by this vector's
    /// context (anything for a zero-sized `T`), all initialized, owned by
    /// no one else.
    pub(crate) unsafe fn adopt_block(&mut self, ptr: NonNull<T>, len: usize) {
        debug_assert!(self.len == 0 && (Self::ZST || self.cap == 0));
        if !Self::ZST {
            self.ptr = ptr;
            self.cap = len;
        }
        self.len = len;
    }

    fn layout(n: usize) -> Option<Layout> {
        Layout::array::<T>(n).ok()
    }

    /// Grow the block to hold `new_cap` elements (`new_cap > cap`).
    #[inline(never)]
    #[cold]
    fn grow_to(&mut self, new_cap: usize) -> Result<(), Oom> {
        debug_assert!(new_cap > self.cap && !Self::ZST);
        let oom = Oom(self.mem);
        let new_layout = Self::layout(new_cap).ok_or(oom)?;
        if new_layout.size() > isize::MAX as usize {
            return Err(oom);
        }
        let ctx = self.mem.ctx();
        let p = if self.cap == 0 {
            ctx.alloc(new_layout, BlockKind::Other)
        } else {
            let old = Self::layout(self.cap).expect("the current block has a layout");
            // SAFETY: `ptr` is this vector's live block of `old`, allocated by
            // the same context; the new size is not 0
            unsafe { ctx.realloc(self.ptr.cast(), old, new_layout.size()) }
        };
        let p = p.ok_or(oom)?;
        self.ptr = p.cast();
        self.cap = new_cap;
        Ok(())
    }

    /// Room for at least `extra` more elements, growing geometrically.
    #[inline(always)]
    pub fn reserve(&mut self, extra: usize) -> Result<(), Oom> {
        if self.cap - self.len >= extra {
            return Ok(());
        }
        self.reserve_slow(extra)
    }

    /// The growing half of [`LVec::reserve`], out of line so the callers
    /// keep only the room test, as with `Vec`.
    #[inline(never)]
    #[cold]
    fn reserve_slow(&mut self, extra: usize) -> Result<(), Oom> {
        let need = self.len.checked_add(extra).ok_or(Oom(self.mem))?;
        self.grow_to(need.max(self.cap.saturating_mul(2)).max(4))
    }

    /// Room for exactly `extra` more elements when it has to grow.
    pub fn reserve_exact(&mut self, extra: usize) -> Result<(), Oom> {
        if self.cap - self.len >= extra {
            return Ok(());
        }
        let need = self.len.checked_add(extra).ok_or(Oom(self.mem))?;
        self.grow_to(need)
    }

    /// Append `v`.
    #[inline(always)]
    pub fn push(&mut self, v: T) -> Result<(), Oom> {
        if self.len == self.cap {
            self.reserve_slow(1)?;
        }
        // SAFETY: `len < cap`, so the slot is inside the block and unused
        unsafe { self.ptr.as_ptr().add(self.len).write(v) };
        self.len += 1;
        Ok(())
    }

    /// Remove and return the last element.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: slot `len` was initialized and is now outside the length,
        // so it is read once
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    /// Drop the elements from `n` on.
    pub fn truncate(&mut self, n: usize) {
        if n >= self.len {
            return;
        }
        let tail = std::ptr::slice_from_raw_parts_mut(
            // SAFETY: `n < len`, inside the block
            unsafe { self.ptr.as_ptr().add(n) },
            self.len - n,
        );
        self.len = n;
        // SAFETY: the tail was initialized and is now outside the length
        unsafe { std::ptr::drop_in_place(tail) };
    }

    /// Drop every element, keeping the block.
    #[inline]
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Insert `v` at `i`, shifting the rest up.
    pub fn insert(&mut self, i: usize, v: T) -> Result<(), Oom> {
        assert!(i <= self.len, "insertion index out of bounds");
        self.reserve(1)?;
        // SAFETY: `i <= len < cap`: the shifted range and slot `i` are inside
        // the block
        unsafe {
            let p = self.ptr.as_ptr().add(i);
            std::ptr::copy(p, p.add(1), self.len - i);
            p.write(v);
        }
        self.len += 1;
        Ok(())
    }

    /// Remove and return the element at `i`, shifting the rest down.
    pub fn remove(&mut self, i: usize) -> T {
        assert!(i < self.len, "removal index out of bounds");
        // SAFETY: `i < len`: slot `i` is initialized and read once, and the
        // shifted range is inside the length
        unsafe {
            let p = self.ptr.as_ptr().add(i);
            let v = p.read();
            std::ptr::copy(p.add(1), p, self.len - i - 1);
            self.len -= 1;
            v
        }
    }

    /// Remove the element at `i`, moving the last one into its place.
    pub fn swap_remove(&mut self, i: usize) -> T {
        let last = self.len - 1;
        self.swap(i, last);
        self.pop().expect("the vector is not empty")
    }

    /// Keep only the elements `f` accepts, in order.
    pub fn retain(&mut self, mut f: impl FnMut(&T) -> bool) {
        let mut kept = 0;
        for i in 0..self.len {
            if f(&self[i]) {
                self.swap(kept, i);
                kept += 1;
            }
        }
        self.truncate(kept);
    }

    /// Move the elements out into a new vector on the same context,
    /// leaving this one empty with no block.
    pub fn take(&mut self) -> LVec<T> {
        std::mem::replace(self, LVec::new(self.mem))
    }

    /// The elements as a boxed slice, giving back the spare capacity.
    #[inline]
    pub fn into_slice(mut self) -> super::LSlice<T> {
        self.shrink_to_fit();
        let me = std::mem::ManuallyDrop::new(self);
        // SAFETY: the block holds exactly `len` initialized elements (or is
        // dangling with none), allocated by `mem`; ownership moves over
        unsafe { super::LSlice::from_raw_parts(me.ptr, me.len, me.mem) }
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
    fn fill_to(&mut self, n: usize, v: T) {
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
        let mut v = LVec::with_capacity(self.mem, self.len)?;
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

/// Growth that ends the process when the allocation fails, as the
/// standard library's `Vec` does. For the places that cannot report a
/// memory error yet.
impl<T> LVec<T> {
    /// [`LVec::push`], ending the process on failure.
    #[inline(always)]
    pub fn push_or_abort(&mut self, v: T) {
        if self.len == self.cap {
            self.reserve_slow_or_abort(1);
        }
        // SAFETY: `len < cap`, so the slot is inside the block and unused
        unsafe { self.ptr.as_ptr().add(self.len).write(v) };
        self.len += 1;
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

impl<T> Deref for LVec<T> {
    type Target = [T];
    #[inline(always)]
    fn deref(&self) -> &[T] {
        // SAFETY: the first `len` elements of the block are initialized
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> DerefMut for LVec<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: the first `len` elements of the block are initialized and
        // the vector is borrowed mutably
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> Drop for LVec<T> {
    fn drop(&mut self) {
        self.clear();
        if !Self::ZST && self.cap != 0 {
            let l = Self::layout(self.cap).expect("the current block has a layout");
            // SAFETY: `ptr` is this vector's live block of `l`, from `mem`
            unsafe { self.mem.ctx().free(self.ptr.cast(), l) };
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> AsRef<[T]> for LVec<T> {
    fn as_ref(&self) -> &[T] {
        self
    }
}

impl<'a, T> IntoIterator for &'a LVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut LVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
