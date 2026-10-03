//! A Lua closure's upvalue storage: inline for small closures, an owned
//! overflow allocation otherwise.

use super::{INLINE_UPVALS_N, LuaClosure};
use crate::runtime::heap::Gc;
use crate::runtime::upvalue::Upvalue;

// one per Lua function value, walked by every sweep: 72 bytes, an 80-byte
// allocation
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<LuaClosure>() == 72);

impl LuaClosure {
    /// View of all upvalues as a `&[Gc<Upvalue>]`. Backed by inline
    /// storage when `upvals_len <= INLINE_UPVALS_N`, else by overflow.
    /// Freshly-derived base pointer for the upvalue storage — same
    /// Stacked Borrows discipline as `Table::array_base`: a cached
    /// pointer into `*self` dies on every `&mut self` entry retag, so
    /// the inline case re-derives through
    /// `UnsafeCell::get()` at each use; the overflow (heap Box) case
    /// keeps the cached pointer whose tag lives outside `*self`.
    /// `upvals_ptr` stays maintained for raw-field consumers.
    #[inline(always)]
    fn upvals_base(&self) -> *mut Gc<Upvalue> {
        if self.upvals_len as usize <= INLINE_UPVALS_N {
            self.inline_storage.get() as *mut Gc<Upvalue>
        } else {
            self.upvals_ptr
        }
    }

    /// View of all upvalues as a `&[Gc<Upvalue>]`. Backed by inline
    /// storage when `upvals_len <= INLINE_UPVALS_N`, else by overflow.
    #[inline(always)]
    pub fn upvals(&self) -> &[Gc<Upvalue>] {
        // SAFETY: `upvals_base` is the inline array or the overflow box, and both hold `upvals_len` handles that the constructor wrote before the closure was adopted (`new_closure_inline` / `set_overflow`); the shared borrow of `self` keeps them from being written while the slice lives
        unsafe { std::slice::from_raw_parts(self.upvals_base(), self.upvals_len as usize) }
    }

    #[inline(always)]
    pub(crate) fn upvals_mut(&mut self) -> &mut [Gc<Upvalue>] {
        // SAFETY: as in `upvals`: `upvals_len` initialised handles at `upvals_base`; `&mut self` makes this the only borrow of them
        unsafe { std::slice::from_raw_parts_mut(self.upvals_base(), self.upvals_len as usize) }
    }

    /// Wire `upvals_ptr` to the active backing storage. Called by the
    /// Heap closure constructors once the LuaClosure is at its stable
    /// heap address (inline_storage's address is only valid after the
    /// Box::new move into the heap).
    /// The overflow case's storage was set by `set_overflow` already.
    pub(crate) fn init_upvals_ptr(&mut self) {
        if self.upvals_len as usize <= INLINE_UPVALS_N {
            self.upvals_ptr = self.inline_storage.get() as *mut Gc<Upvalue>;
        }
    }

    /// Hand a closure with more than `INLINE_UPVALS_N` upvalues its storage.
    pub(crate) fn set_overflow(&mut self, upvals: Box<[Gc<Upvalue>]>) {
        debug_assert_eq!(upvals.len(), self.upvals_len as usize);
        debug_assert!(upvals.len() > INLINE_UPVALS_N);
        self.upvals_ptr = Box::into_raw(upvals) as *mut Gc<Upvalue>;
    }
}

impl Drop for LuaClosure {
    fn drop(&mut self) {
        let n = self.upvals_len as usize;
        if n > INLINE_UPVALS_N {
            // SAFETY: an overflow closure's `upvals_ptr` came from
            // `Box::into_raw` of a slice of `n` (`set_overflow`)
            drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.upvals_ptr, n)) });
        }
    }
}
