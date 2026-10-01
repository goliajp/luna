//! The handle to a GC-managed object.

use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

/// `Copy` handle to a heap-allocated GC-managed object. Layout is a single
/// `NonNull<T>`; the GC walks reachability via root scanning and intrusive
/// linkage on [`GcHeader`], not via reference counts.
pub struct Gc<T> {
    ptr: NonNull<T>,
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Gc<T> {}

impl<T> Gc<T> {
    #[doc(hidden)]
    pub fn from_ptr(p: *mut T) -> Gc<T> {
        Gc {
            ptr: NonNull::new(p).expect("gc pointer must be non-null"),
        }
    }

    /// [`Self::from_ptr`] without the null check.
    ///
    /// # Safety
    /// `p` is not null.
    #[inline(always)]
    pub(crate) unsafe fn from_ptr_unchecked(p: *mut T) -> Gc<T> {
        // SAFETY: the caller's contract
        Gc {
            ptr: unsafe { NonNull::new_unchecked(p) },
        }
    }

    /// Raw pointer to the referent. Always non-null; valid for the lifetime
    /// of the [`Heap`] that allocated it as long as the object is reachable.
    pub fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Pointer-identity equality (PUC `rawequal` for reference types).
    pub fn ptr_eq(self, other: Gc<T>) -> bool {
        self.ptr == other.ptr
    }

    /// SAFETY: caller must ensure no other live reference to the object and
    /// no collect() while the borrow is held (single-threaded runtime).
    ///
    /// `#[doc(hidden)]` so the documented public surface needs no `unsafe`:
    /// embedders should not see this in rustdoc. The safe path for mutating
    /// freshly-allocated tables is the `TableBuilder` / `vm.table_of(...)` API.
    /// Cross-crate access from `luna` (e.g. `jit_backend`, `capi`) keeps
    /// working — `#[doc(hidden)] pub` doesn't demote visibility, just docs.
    #[doc(hidden)]
    pub unsafe fn as_mut<'a>(self) -> &'a mut T {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { &mut *self.ptr.as_ptr() }
    }
}

impl<T> Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gc({:p})", self.ptr.as_ptr())
    }
}
