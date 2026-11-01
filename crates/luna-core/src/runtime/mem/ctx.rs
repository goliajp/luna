//! The allocation context: where a Vm's memory comes from.

use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

pub(super) use super::kind::{BlockKind, KINDS};

/// A host allocation function with PUC's `lua_Alloc` contract: a new
/// block has `ptr` null and `osize` the kind of object it is for; a
/// resize passes the old block and size; `nsize` 0 frees `ptr` (and
/// must return null). A null result for `nsize > 0` is a failure.
pub type RawAllocFn = unsafe extern "C" fn(
    ud: *mut c_void,
    ptr: *mut c_void,
    osize: usize,
    nsize: usize,
) -> *mut c_void;

#[derive(Clone, Copy)]
pub(super) enum Mode {
    System,
    Raw,
    Policy,
}

/// Where a Vm's memory comes from, shared by every container of the Vm
/// through a [`MemRef`]. Owned jointly by the heap and the Vm
/// ([`MemOwner`](super::MemOwner)), so it outlives every container either
/// of them holds. A context serves one heap: Vms given clones of one
/// owner each get a context of their own, sharing the owner's budget.
// laid out in declaration order: with the kept-block lists at offset 0 the
// allocation path spends two more instructions indexing them
#[repr(C)]
pub struct MemCtx {
    pub(super) mode: Mode,
    /// the host function or policy, and the bytes counted, which the
    /// contexts made from one owner share
    pub(super) budget: NonNull<super::budget::Budget>,
    pub(super) codes: [u8; KINDS],
    pub(super) owners: super::sync::Count,
    /// whether a heap has taken the context; another heap given it makes a
    /// context of its own
    pub(super) claimed: super::sync::Once,
    /// the fixed "not enough memory" string a memory error carries (PUC
    /// `memerrmsg`), made by the heap that claimed the context; null before
    /// it is made and after that heap is dropped
    pub(super) memerr: Cell<*mut crate::runtime::LuaStr>,
    /// memory errors raised so far, for telling a memory error from a
    /// program's error with the same text
    pub(super) oom_raised: Cell<u64>,
    /// freed blocks kept for reuse (`Mode::System`, see `pool.rs` and
    /// `small.rs`)
    pub(super) pool: RefCell<super::pool::Pool>,
    pub(super) small: RefCell<super::small::Small>,
    /// the block small objects are cut from while the state is built
    pub(super) arena: super::arena::Arena,
}

/// A handle to a [`MemCtx`], kept by each container to free and grow its
/// block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemRef(pub(super) NonNull<MemCtx>);

/// An allocation the context could not make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Oom(pub(crate) MemRef);

impl Oom {
    /// Give up where no memory error can be returned: see
    /// `oom_abort`.
    #[cold]
    #[inline(never)]
    pub(crate) fn fail(self) -> ! {
        super::oom_abort(std::alloc::Layout::new::<u8>())
    }
}

impl MemRef {
    /// The context.
    #[inline(always)]
    pub(crate) fn ctx(self) -> &'static MemCtx {
        // SAFETY: a `MemRef` is only made from a live context, which its
        // owners keep alive past every container holding the handle; the
        // context is only ever used through shared references
        unsafe { self.0.as_ref() }
    }
}

impl MemCtx {
    /// A new block of `layout` for `kind`, or `None` when the allocation
    /// fails. `layout.size()` is not 0.
    #[inline(always)]
    pub(crate) fn alloc(&self, layout: Layout, kind: BlockKind) -> Option<NonNull<u8>> {
        debug_assert!(layout.size() != 0);
        match &self.mode {
            Mode::System if super::small::pooled(layout) => self.alloc_small(layout),
            Mode::System if super::pool::pooled(layout.size()) => self.alloc_pooled(layout),
            // SAFETY: the size is not 0
            Mode::System => NonNull::new(unsafe { std::alloc::alloc(layout) }),
            _ => self.alloc_slow(layout, kind),
        }
    }

    /// [`MemCtx::alloc`] with the block's bytes zeroed.
    #[inline(always)]
    pub(crate) fn alloc_zeroed(&self, layout: Layout, kind: BlockKind) -> Option<NonNull<u8>> {
        debug_assert!(layout.size() != 0);
        match &self.mode {
            Mode::System if super::small::pooled(layout) => {
                let p = self.alloc_small(layout)?;
                // SAFETY: `p` is a block of `layout.size()` bytes
                unsafe { p.as_ptr().write_bytes(0, layout.size()) };
                Some(p)
            }
            Mode::System if super::pool::pooled(layout.size()) => self.alloc_pooled_zeroed(layout),
            // SAFETY: the size is not 0
            Mode::System => NonNull::new(unsafe { std::alloc::alloc_zeroed(layout) }),
            _ => {
                let p = self.alloc_slow(layout, kind)?;
                // SAFETY: `p` is a fresh block of `layout.size()` bytes
                unsafe { p.as_ptr().write_bytes(0, layout.size()) };
                Some(p)
            }
        }
    }

    /// `p`, a block of `layout` from this context, resized to `new`
    /// bytes, or `None` (and `p` untouched) when that fails.
    ///
    /// # Safety
    /// `p` was allocated by this context with `layout` and is not freed;
    /// `new` is not 0.
    #[inline(always)]
    pub(crate) unsafe fn realloc(
        &self,
        p: NonNull<u8>,
        layout: Layout,
        new: usize,
    ) -> Option<NonNull<u8>> {
        debug_assert!(new != 0);
        match &self.mode {
            // SAFETY: the caller's contract
            Mode::System if self.arena.holds(p) => unsafe { self.move_from_arena(p, layout, new) },
            // SAFETY: the caller's contract
            Mode::System if new <= super::resize::SMALL_MOVE => unsafe {
                super::resize::move_small(p, layout, new)
            },
            // SAFETY: the caller's contract
            Mode::System if super::pool::pooled(new) || super::pool::pooled(layout.size()) => unsafe {
                self.move_pooled(p, layout, new)
            },
            // SAFETY: the caller's contract
            Mode::System => NonNull::new(unsafe { std::alloc::realloc(p.as_ptr(), layout, new) }),
            // SAFETY: the caller's contract
            _ => unsafe { self.realloc_slow(p, layout, new) },
        }
    }

    /// Give back `p`, a block of `layout` from this context.
    ///
    /// # Safety
    /// `p` was allocated by this context with `layout` and is not freed.
    #[inline(always)]
    pub(crate) unsafe fn free(&self, p: NonNull<u8>, layout: Layout) {
        match &self.mode {
            // SAFETY: the caller's contract: a system block of `layout`
            // nothing uses, kept or freed
            Mode::System if super::small::pooled(layout) => unsafe {
                if !self.arena.holds(p) && !self.small.borrow_mut().keep(p, layout) {
                    std::alloc::dealloc(p.as_ptr(), layout)
                }
            },
            Mode::System if super::pool::pooled(layout.size()) => {
                if !self.pool.borrow_mut().keep(p, layout) {
                    // SAFETY: the caller's contract
                    unsafe { std::alloc::dealloc(p.as_ptr(), layout) }
                }
            }
            // SAFETY: the caller's contract
            Mode::System => unsafe { std::alloc::dealloc(p.as_ptr(), layout) },
            // SAFETY: the caller's contract
            _ => unsafe { self.free_slow(p, layout) },
        }
    }

    /// Set the string memory errors carry; the heap fixes it so it is never
    /// collected, and calls [`MemCtx::forget_memerr`] before it frees it.
    pub(crate) fn set_memerr(&self, s: crate::runtime::Gc<crate::runtime::LuaStr>) {
        self.memerr.set(s.as_ptr());
    }

    /// The heap that made the memory error's string is freeing it.
    pub(crate) fn forget_memerr(&self) {
        self.memerr.set(std::ptr::null_mut());
    }

    /// A memory error is being raised: count it and give its message.
    pub(crate) fn raise_oom(&self) -> crate::runtime::Gc<crate::runtime::LuaStr> {
        self.oom_raised.set(self.oom_raised.get() + 1);
        let p = self.memerr.get();
        assert!(
            !p.is_null(),
            "the heap makes its memory error message first"
        );
        // SAFETY: `memerr` is a string the context's one heap made and
        // fixed, which lives until that heap forgets it before freeing it
        unsafe { crate::runtime::Gc::from_ptr(p) }
    }

    /// Memory errors raised so far.
    pub(crate) fn oom_raised(&self) -> u64 {
        self.oom_raised.get()
    }
}
