//! `LBox<T>` and `LSlice<T>`: owned blocks from a [`MemCtx`].

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

use super::LVec;
use super::ctx::{BlockKind, MemRef, Oom};

/// A `Box<T>` whose block comes from a Vm's allocation context.
pub struct LBox<T> {
    ptr: NonNull<T>,
    mem: MemRef,
    _own: PhantomData<T>,
}

impl<T> LBox<T> {
    const ALIGN_OK: () = assert!(std::mem::align_of::<T>() <= 8);

    /// Move `v` into a new block of kind `kind`.
    pub fn new_kind(mem: MemRef, v: T, kind: BlockKind) -> Result<LBox<T>, Oom> {
        let () = Self::ALIGN_OK;
        let layout = Layout::new::<T>();
        let ptr: NonNull<T> = if layout.size() == 0 {
            NonNull::dangling()
        } else {
            mem.ctx().alloc(layout, kind).ok_or(Oom(mem))?.cast()
        };
        // SAFETY: `ptr` is a fresh block for one `T` (or dangling for a
        // zero-sized `T`)
        unsafe { ptr.as_ptr().write(v) };
        Ok(LBox {
            ptr,
            mem,
            _own: PhantomData,
        })
    }

    /// Move `v` into a new block.
    pub fn new(mem: MemRef, v: T) -> Result<LBox<T>, Oom> {
        LBox::new_kind(mem, v, BlockKind::Other)
    }

    /// The handle the box frees through.
    #[inline(always)]
    pub fn mem(&self) -> MemRef {
        self.mem
    }
}

impl<T> Deref for LBox<T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        // SAFETY: the block holds an initialized `T` the box owns
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for LBox<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the block holds an initialized `T` and the box is borrowed
        // mutably
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for LBox<T> {
    fn drop(&mut self) {
        // SAFETY: the box owns the initialized `T` in its block; after this
        // the block is freed and never read
        unsafe {
            std::ptr::drop_in_place(self.ptr.as_ptr());
            let l = Layout::new::<T>();
            if l.size() != 0 {
                self.mem.ctx().free(self.ptr.cast(), l);
            }
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// A `Box<[T]>` whose block comes from a Vm's allocation context.
pub struct LSlice<T> {
    ptr: NonNull<T>,
    len: usize,
    mem: MemRef,
    _own: PhantomData<T>,
}

impl<T> LSlice<T> {
    /// An empty slice; allocates nothing.
    pub fn empty(mem: MemRef) -> LSlice<T> {
        LSlice {
            ptr: NonNull::dangling(),
            len: 0,
            mem,
            _own: PhantomData,
        }
    }

    /// Adopt a block of exactly `len` initialized elements.
    ///
    /// # Safety
    /// `ptr` is a block of `len` elements allocated by `mem` (dangling when
    /// the block's size is 0), all initialized, owned by no one else.
    pub(crate) unsafe fn from_raw_parts(ptr: NonNull<T>, len: usize, mem: MemRef) -> LSlice<T> {
        LSlice {
            ptr,
            len,
            mem,
            _own: PhantomData,
        }
    }

    /// The handle the slice frees through.
    #[inline(always)]
    pub fn mem(&self) -> MemRef {
        self.mem
    }

    /// The block, no longer owned: [`LSlice::from_raw_parts`] with the
    /// same length and handle takes it back.
    pub(crate) fn into_raw_parts(self) -> NonNull<T> {
        std::mem::ManuallyDrop::new(self).ptr
    }

    /// Back into a vector (length and capacity both `len`).
    pub fn into_vec(self) -> LVec<T> {
        let me = std::mem::ManuallyDrop::new(self);
        let mut v = LVec::new(me.mem);
        if me.len != 0 || std::mem::size_of::<T>() == 0 {
            // SAFETY: the block holds `len` initialized elements and was
            // allocated by `mem` for exactly that many
            unsafe { v.adopt_block(me.ptr, me.len) };
        }
        v
    }
}

impl<T> LSlice<T> {
    /// The items of `it` in a block of exactly their number.
    pub fn collect_exact(
        mem: MemRef,
        it: impl ExactSizeIterator<Item = T>,
    ) -> Result<LSlice<T>, Oom> {
        let mut v = LVec::with_capacity(mem, it.len())?;
        for x in it {
            v.push(x)?;
        }
        Ok(v.into_slice())
    }
}

impl<T: Clone> LSlice<T> {
    /// A slice on `mem` holding clones of `s`.
    pub fn from_slice(mem: MemRef, s: &[T]) -> Result<LSlice<T>, Oom> {
        Ok(LVec::from_slice(mem, s)?.into_slice())
    }

    /// A copy on the same context.
    pub fn try_clone(&self) -> Result<LSlice<T>, Oom> {
        LSlice::from_slice(self.mem, self)
    }
}

impl<T> Deref for LSlice<T> {
    type Target = [T];
    #[inline(always)]
    fn deref(&self) -> &[T] {
        // SAFETY: the block holds `len` initialized elements
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> DerefMut for LSlice<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: the block holds `len` initialized elements and the slice is
        // borrowed mutably
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> Drop for LSlice<T> {
    fn drop(&mut self) {
        drop(std::mem::replace(self, LSlice::empty(self.mem)).into_vec());
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LSlice<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<'a, T> IntoIterator for &'a LSlice<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
