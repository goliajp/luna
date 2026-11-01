//! The allocation paths outside the system allocator: a host's function or a
//! policy, and the bytes they count.

use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use super::MemoryPolicy;
use super::ctx::{BlockKind, MemCtx, Mode, RawAllocFn};

/// What the contexts made from one [`MemOwner`](super::MemOwner) share:
/// every Vm given a clone of the owner allocates against one budget.
pub(super) struct Budget {
    /// the host function and its `ud` (`Mode::Raw`); `lua_setallocf`
    /// replaces them
    pub(super) raw: Cell<Option<(RawAllocFn, *mut c_void)>>,
    /// the policy asked before each allocation (`Mode::Policy`)
    pub(super) policy: Option<RefCell<Box<dyn MemoryPolicy>>>,
    /// bytes allocated and not freed by all the contexts, kept outside
    /// `Mode::System`
    pub(super) total: Cell<usize>,
    /// contexts using the budget; the last one frees it
    pub(super) users: Cell<usize>,
}

impl Budget {
    fn policy(&self) -> std::cell::RefMut<'_, Box<dyn MemoryPolicy>> {
        self.policy
            .as_ref()
            .expect("a policy context has a policy")
            .borrow_mut()
    }
}

impl MemCtx {
    #[inline(always)]
    pub(super) fn budget(&self) -> &Budget {
        // SAFETY: a context holds one use of its budget, which is freed only
        // after its last user
        unsafe { self.budget.as_ref() }
    }

    #[inline(never)]
    pub(super) fn alloc_slow(&self, layout: Layout, kind: BlockKind) -> Option<NonNull<u8>> {
        let size = layout.size();
        let p = match &self.mode {
            Mode::System => unreachable!(),
            Mode::Raw => {
                let code = self.codes[kind as usize] as usize;
                // SAFETY: the host's function, called as PUC calls it for a
                // new block
                NonNull::new(unsafe { self.call_raw(std::ptr::null_mut(), code, size) })?
            }
            Mode::Policy => {
                let b = self.budget();
                if !b.policy().allow(0, size, kind, b.total.get()) {
                    return None;
                }
                // SAFETY: the size is not 0
                NonNull::new(unsafe { std::alloc::alloc(layout) })?
            }
        };
        self.budget().total.set(self.budget().total.get() + size);
        Some(p)
    }

    #[inline(never)]
    pub(super) unsafe fn realloc_slow(
        &self,
        p: NonNull<u8>,
        layout: Layout,
        new: usize,
    ) -> Option<NonNull<u8>> {
        let old = layout.size();
        let q = match &self.mode {
            Mode::System => unreachable!(),
            // SAFETY: `p` is a live block of `old` bytes from this function
            // or one it replaced, which PUC requires to accept it
            Mode::Raw => NonNull::new(unsafe { self.call_raw(p.as_ptr(), old, new) })?,
            Mode::Policy => {
                let b = self.budget();
                if new > old && !b.policy().allow(old, new, BlockKind::Other, b.total.get()) {
                    return None;
                }
                // SAFETY: the caller's contract
                let q = NonNull::new(unsafe { std::alloc::realloc(p.as_ptr(), layout, new) })?;
                if new < old {
                    b.policy().freed(old - new);
                }
                q
            }
        };
        self.budget()
            .total
            .set(self.budget().total.get() - old + new);
        Some(q)
    }

    #[inline(never)]
    pub(super) unsafe fn free_slow(&self, p: NonNull<u8>, layout: Layout) {
        match &self.mode {
            Mode::System => unreachable!(),
            Mode::Raw => {
                // SAFETY: `p` is a live block of this size from the host's
                // function (or one it replaced); a free returns null
                unsafe { self.call_raw(p.as_ptr(), layout.size(), 0) };
            }
            Mode::Policy => {
                // SAFETY: the caller's contract
                unsafe { std::alloc::dealloc(p.as_ptr(), layout) };
                self.budget().policy().freed(layout.size());
            }
        }
        self.budget()
            .total
            .set(self.budget().total.get() - layout.size());
    }

    /// # Safety
    /// As the host's `lua_Alloc` requires of `ptr` and the sizes.
    unsafe fn call_raw(&self, ptr: *mut u8, osize: usize, nsize: usize) -> *mut u8 {
        let (f, ud) = self
            .budget()
            .raw
            .get()
            .expect("a raw context has a function");
        // SAFETY: the caller's contract
        unsafe { f(ud, ptr.cast(), osize, nsize).cast() }
    }

    /// Bytes allocated and not freed, when the context counts them (a
    /// host function or a policy); `None` with the system allocator.
    pub fn in_use(&self) -> Option<usize> {
        match self.mode {
            Mode::System => None,
            _ => Some(self.budget().total.get()),
        }
    }

    /// Count `n` more bytes the host allocated through the same function
    /// on the Vm's behalf (the C API's state records), so the count
    /// covers them as PUC's does.
    pub fn add_external(&self, n: usize) {
        self.budget().total.set(self.budget().total.get() + n);
    }

    /// The host function and its `ud`, when the context has one.
    pub fn raw_alloc(&self) -> Option<(RawAllocFn, *mut c_void)> {
        self.budget().raw.get()
    }

    /// Replace the host function (PUC `lua_setallocf`): blocks allocated
    /// so far are resized and freed through the new one.
    ///
    /// # Safety
    /// `f` accepts every live block of the previous function, with `ud`.
    pub(crate) unsafe fn set_raw_alloc(&self, f: RawAllocFn, ud: *mut c_void) {
        assert!(
            matches!(self.mode, Mode::Raw),
            "the context has a host function"
        );
        self.budget().raw.set(Some((f, ud)));
    }
}
