//! The allocation context: where a Vm's memory comes from.

use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use crate::version::LuaVersion;

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

/// What a block of memory is for. A new block's kind reaches a host
/// allocation function as its `osize`, in the dialect's numbering
/// (PUC's object type tags; 0 for anything that is not an object).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BlockKind {
    /// not an object: arrays, buffers, records
    Other,
    /// a string
    Str,
    /// a table
    Table,
    /// a Lua function (closure)
    LuaFn,
    /// a native function
    NativeFn,
    /// a full userdata
    Userdata,
    /// a thread (coroutine)
    Thread,
    /// a function prototype
    Proto,
    /// an upvalue
    Upvalue,
}

const KINDS: usize = 9;

/// The `osize` each [`BlockKind`] has for a new block in dialect `v`.
pub(super) fn kind_codes(v: LuaVersion) -> [u8; KINDS] {
    match v {
        LuaVersion::Lua51 => [0; KINDS],
        LuaVersion::Lua52 => [0, 4, 5, 6, 6, 7, 8, 9, 10],
        // 5.3 upvalues are reference counted, not objects
        LuaVersion::Lua53 => [0, 4, 5, 6, 6, 7, 8, 9, 0],
        _ => [0, 4, 5, 6, 6, 7, 8, 10, 9],
    }
}

/// A safe way for a Rust host to watch and limit a Vm's memory. The
/// memory itself still comes from the system allocator.
pub trait MemoryPolicy {
    /// A block is about to grow from `old` bytes to `new` (`old` is 0 for
    /// a new block); `in_use` is what the Vm has allocated so far. Return
    /// false to make the allocation fail.
    fn allow(&mut self, old: usize, new: usize, kind: BlockKind, in_use: usize) -> bool;
    /// `size` bytes were given back.
    fn freed(&mut self, size: usize) {
        let _ = size;
    }
}

/// A [`MemoryPolicy`] that refuses any growth past `limit` bytes in use.
#[derive(Clone, Copy, Debug)]
pub struct MemoryLimit(pub usize);

impl MemoryPolicy for MemoryLimit {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, in_use: usize) -> bool {
        in_use - old.min(in_use) + new <= self.0
    }
}

pub(super) enum Mode {
    System,
    Raw,
    Policy(RefCell<Box<dyn MemoryPolicy>>),
}

/// Where a Vm's memory comes from, shared by every container of the Vm
/// through a [`MemRef`]. Owned jointly by the heap and the Vm
/// ([`MemOwner`](super::MemOwner)), so it outlives every container either
/// of them holds.
pub struct MemCtx {
    pub(super) mode: Mode,
    /// the host function and its `ud` (`Mode::Raw`); `lua_setallocf`
    /// replaces them
    pub(super) raw: Cell<Option<(RawAllocFn, *mut c_void)>>,
    pub(super) codes: [u8; KINDS],
    /// bytes allocated and not freed, kept outside `Mode::System`
    pub(super) total: Cell<usize>,
    pub(super) owners: Cell<usize>,
    /// the fixed "not enough memory" string a memory error carries (PUC
    /// `memerrmsg`); null until the heap has made it
    pub(super) memerr: Cell<*mut crate::runtime::LuaStr>,
    /// memory errors raised so far, for telling a memory error from a
    /// program's error with the same text
    pub(super) oom_raised: Cell<u64>,
    /// freed blocks kept for reuse (`Mode::System`, see `pool.rs`)
    pub(super) pool: RefCell<super::pool::Pool>,
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

    #[inline(never)]
    fn alloc_slow(&self, layout: Layout, kind: BlockKind) -> Option<NonNull<u8>> {
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

    #[inline(never)]
    unsafe fn realloc_slow(
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
                NonNull::new(unsafe { std::alloc::realloc(p.as_ptr(), layout, new) })?
            }
        };
        self.total.set(self.total.get() - old + new);
        Some(q)
    }

    /// Give back `p`, a block of `layout` from this context.
    ///
    /// # Safety
    /// `p` was allocated by this context with `layout` and is not freed.
    #[inline(always)]
    pub(crate) unsafe fn free(&self, p: NonNull<u8>, layout: Layout) {
        match &self.mode {
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

    #[inline(never)]
    unsafe fn free_slow(&self, p: NonNull<u8>, layout: Layout) {
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

    /// Set the string memory errors carry; the heap fixes it so it is never
    /// collected.
    pub(crate) fn set_memerr(&self, s: crate::runtime::Gc<crate::runtime::LuaStr>) {
        self.memerr.set(s.as_ptr());
    }

    /// A memory error is being raised: count it and give its message.
    pub(crate) fn raise_oom(&self) -> crate::runtime::Gc<crate::runtime::LuaStr> {
        self.oom_raised.set(self.oom_raised.get() + 1);
        let p = self.memerr.get();
        assert!(
            !p.is_null(),
            "the heap makes its memory error message first"
        );
        // SAFETY: `memerr` is a string the heap made and fixed, which lives
        // as long as the heap and so as long as this context's owners
        unsafe { crate::runtime::Gc::from_ptr(p) }
    }

    /// Memory errors raised so far.
    pub(crate) fn oom_raised(&self) -> u64 {
        self.oom_raised.get()
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
    #[doc(hidden)]
    pub unsafe fn set_raw_alloc(&self, f: RawAllocFn, ud: *mut c_void) {
        assert!(
            matches!(self.mode, Mode::Raw),
            "the context has a host function"
        );
        self.raw.set(Some((f, ud)));
    }
}
