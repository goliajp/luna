//! `LAny`: a `Box<dyn Any>` whose block comes from a [`MemCtx`].

use std::alloc::Layout;
use std::any::{Any, TypeId};
use std::ptr::NonNull;

use super::ctx::{BlockKind, MemRef, Oom};

/// An owned value of any `'static` type in a block from a Vm's
/// allocation context, recovered by type as with `Box<dyn Any>`
/// (`std` cannot put an unsized `dyn Any` in a block of its own choosing).
pub struct LAny {
    ptr: NonNull<u8>,
    layout: Layout,
    type_id: TypeId,
    /// drops the value in place
    drop_fn: unsafe fn(*mut u8),
    mem: MemRef,
}

/// # Safety
/// `p` points at an initialized `T` that is not used again.
unsafe fn drop_as<T>(p: *mut u8) {
    // SAFETY: the caller's contract
    unsafe { std::ptr::drop_in_place(p.cast::<T>()) }
}

impl LAny {
    /// Move `v` into a new block of kind `kind`.
    pub fn new<T: Any>(mem: MemRef, v: T, kind: BlockKind) -> Result<LAny, Oom> {
        const { assert!(std::mem::align_of::<T>() <= 16) };
        let layout = Layout::new::<T>();
        let ptr = if layout.size() == 0 {
            NonNull::<T>::dangling().cast()
        } else {
            mem.ctx().alloc(layout, kind).ok_or(Oom(mem))?
        };
        // SAFETY: `ptr` is a fresh block for one `T` (or dangling for a
        // zero-sized `T`)
        unsafe { ptr.cast::<T>().as_ptr().write(v) };
        Ok(LAny {
            ptr,
            layout,
            type_id: TypeId::of::<T>(),
            drop_fn: drop_as::<T>,
            mem,
        })
    }

    /// The type of the value.
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Whether the value is a `T`.
    pub fn is<T: Any>(&self) -> bool {
        self.type_id == TypeId::of::<T>()
    }

    /// The value, if it is a `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        // SAFETY: the block holds an initialized value of the type
        // `type_id` names, here `T`
        (self.type_id == TypeId::of::<T>()).then(|| unsafe { self.ptr.cast::<T>().as_ref() })
    }

    /// The value, if it is a `T`, for a write.
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        // SAFETY: as `downcast_ref`; `&mut self` makes the access exclusive
        (self.type_id == TypeId::of::<T>()).then(|| unsafe { self.ptr.cast::<T>().as_mut() })
    }

    /// The block's address, which stays the same while the value lives.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

impl Drop for LAny {
    fn drop(&mut self) {
        // SAFETY: the block holds the initialized value `drop_fn` was made
        // for; after this it is freed with the layout it was allocated with
        unsafe {
            (self.drop_fn)(self.ptr.as_ptr());
            if self.layout.size() != 0 {
                self.mem.ctx().free(self.ptr, self.layout);
            }
        }
    }
}
