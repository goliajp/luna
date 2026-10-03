//! The handle to a GC-managed object.

use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;

use super::GcHeader;

/// `Copy` handle to a heap-allocated GC-managed object. Layout is a single
/// `NonNull<T>`; the GC walks reachability via root scanning and intrusive
/// linkage on [`GcHeader`](super::GcHeader), not via reference counts.
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
    /// Handle to the object at `p`. Panics when `p` is null.
    ///
    /// # Safety
    /// `p` points at a live object of type `T` that a [`Heap`](super::Heap)
    /// allocated and still manages, and whoever uses the handle keeps that
    /// object reachable while doing so: [`Deref`] reads through it.
    #[doc(hidden)]
    pub unsafe fn from_ptr(p: *mut T) -> Gc<T> {
        Gc {
            ptr: NonNull::new(p).expect("gc pointer must be non-null"),
        }
    }

    /// [`Self::from_ptr`] without the null check.
    ///
    /// # Safety
    /// As for [`Self::from_ptr`], and `p` is not null.
    #[inline(always)]
    pub(crate) unsafe fn from_ptr_unchecked(p: *mut T) -> Gc<T> {
        Gc {
            // SAFETY: the caller's contract
            ptr: unsafe { NonNull::new_unchecked(p) },
        }
    }

    /// Raw pointer to the referent. Always non-null; valid for the lifetime
    /// of the [`Heap`](super::Heap) that allocated it as long as the object is reachable.
    pub fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Pointer-identity equality (PUC `rawequal` for reference types).
    pub fn ptr_eq(self, other: Gc<T>) -> bool {
        self.ptr == other.ptr
    }

    /// Exclusive borrow of the referent.
    ///
    /// # Safety
    /// The object is still allocated (reachable from the roots, or not yet
    /// past a collect), no other reference to it is live while the returned
    /// borrow is, and no collect runs while the borrow is held.
    ///
    /// `#[doc(hidden)]` so the documented public surface needs no `unsafe`:
    /// embedders should not see this in rustdoc. The safe path for mutating
    /// freshly-allocated tables is the `TableBuilder` / `vm.table_of(...)` API.
    /// Cross-crate access from `luna` (e.g. `jit_backend`, `capi`) keeps
    /// working — `#[doc(hidden)] pub` doesn't demote visibility, just docs.
    #[doc(hidden)]
    pub unsafe fn as_mut<'a>(self) -> &'a mut T {
        // SAFETY: `ptr` is non-null and points at an object of the heap that allocated it; the caller's contract above keeps that object unfreed and unaliased for as long as the returned borrow lives
        unsafe { &mut *self.ptr.as_ptr() }
    }
}

/// A type the GC allocates: `#[repr(C)]` with its [`GcHeader`] as the first
/// field, so a pointer to the object is a pointer to its header. Sealed: only
/// the runtime's own object types implement it.
pub trait GcObject: sealed::Sealed {}

mod sealed {
    pub trait Sealed {}
}

macro_rules! gc_objects {
    ($($t:ty),* $(,)?) => {$(
        impl sealed::Sealed for $t {}
        impl GcObject for $t {}
        const _: () = assert!(std::mem::offset_of!($t, hdr) == 0);
    )*};
}

gc_objects!(
    crate::runtime::LuaStr,
    crate::runtime::Table,
    crate::runtime::Proto,
    crate::runtime::LuaClosure,
    crate::runtime::function::Upvalue,
    crate::runtime::NativeClosure,
    crate::runtime::Coro,
    crate::runtime::Userdata,
);

impl<T: GcObject> Gc<T> {
    /// The object's GC header.
    #[inline(always)]
    pub(crate) fn header(self) -> *mut GcHeader {
        self.ptr.as_ptr().cast()
    }
}

impl<T> Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: `ptr` is non-null and points at an object its heap allocated; whoever holds a `Gc` keeps the object reachable (rooted or held by a reachable object) while using it, so no collect frees it during the borrow of `self`, and `as_mut` callers end their exclusive borrow before the next shared read
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gc({:p})", self.ptr.as_ptr())
    }
}
