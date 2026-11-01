//! The allocation paths outside the system allocator: a host's function or a
//! policy, and the bytes they count.

use std::alloc::Layout;
use std::ffi::c_void;
use std::ptr::NonNull;

use super::ctx::{BlockKind, MemCtx, Mode, RawAllocFn};

impl MemCtx {
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
            Mode::Policy(p) => {
                if !p.borrow_mut().allow(0, size, kind, self.total.get()) {
                    return None;
                }
                // SAFETY: the size is not 0
                NonNull::new(unsafe { std::alloc::alloc(layout) })?
            }
        };
        self.total.set(self.total.get() + size);
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
            Mode::Policy(pol) => {
                if new > old
                    && !pol
                        .borrow_mut()
                        .allow(old, new, BlockKind::Other, self.total.get())
                {
                    return None;
                }
                // SAFETY: the caller's contract
                let q = NonNull::new(unsafe { std::alloc::realloc(p.as_ptr(), layout, new) })?;
                if new < old {
                    pol.borrow_mut().freed(old - new);
                }
                q
            }
        };
        self.total.set(self.total.get() - old + new);
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
            Mode::Policy(pol) => {
                // SAFETY: the caller's contract
                unsafe { std::alloc::dealloc(p.as_ptr(), layout) };
                pol.borrow_mut().freed(layout.size());
            }
        }
        self.total.set(self.total.get() - layout.size());
    }

    /// # Safety
    /// As the host's `lua_Alloc` requires of `ptr` and the sizes.
    unsafe fn call_raw(&self, ptr: *mut u8, osize: usize, nsize: usize) -> *mut u8 {
        let (f, ud) = self.raw.get().expect("a raw context has a function");
        // SAFETY: the caller's contract
        unsafe { f(ud, ptr.cast(), osize, nsize).cast() }
    }

    /// Bytes allocated and not freed, when the context counts them (a
    /// host function or a policy); `None` with the system allocator.
    pub fn in_use(&self) -> Option<usize> {
        match self.mode {
            Mode::System => None,
            _ => Some(self.total.get()),
        }
    }

    /// Count `n` more bytes the host allocated through the same function
    /// on the Vm's behalf (the C API's state records), so the count
    /// covers them as PUC's does.
    pub fn add_external(&self, n: usize) {
        self.total.set(self.total.get() + n);
    }

    /// The host function and its `ud`, when the context has one.
    pub fn raw_alloc(&self) -> Option<(RawAllocFn, *mut c_void)> {
        self.raw.get()
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
        self.raw.set(Some((f, ud)));
    }
}
