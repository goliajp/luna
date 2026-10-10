//! The owners of a [`MemCtx`]: the context lives until the last one goes.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use super::ctx::{MemCtx, MemRef, MemoryPolicy, Mode, RawAllocFn, kind_codes};
use crate::version::LuaVersion;

/// One owner of a [`MemCtx`]; the context is freed with its last owner.
pub struct MemOwner(MemRef);

impl MemOwner {
    fn with_mode(mode: Mode, raw: Option<(RawAllocFn, *mut c_void)>, v: LuaVersion) -> MemOwner {
        let ctx = Box::new(MemCtx {
            mode,
            raw: Cell::new(raw),
            codes: kind_codes(v),
            total: Cell::new(0),
            owners: Cell::new(1),
            memerr: Cell::new(std::ptr::null_mut()),
            oom_raised: Cell::new(0),
        });
        MemOwner(MemRef(NonNull::from(Box::leak(ctx))))
    }

    /// A context on the system allocator.
    pub fn system() -> MemOwner {
        MemOwner::with_mode(Mode::System, None, LuaVersion::Lua54)
    }

    /// A context whose memory comes from `f` with `ud`, numbering the
    /// kinds of new blocks as dialect `v` does.
    ///
    /// # Safety
    /// `f` follows PUC's `lua_Alloc` contract and may be called with
    /// `ud` until the last owner is dropped, which frees through it every
    /// block still allocated.
    pub unsafe fn raw(f: RawAllocFn, ud: *mut c_void, v: LuaVersion) -> MemOwner {
        MemOwner::with_mode(Mode::Raw, Some((f, ud)), v)
    }

    /// A context on the system allocator that asks `policy` first.
    pub fn policy(policy: Box<dyn MemoryPolicy>) -> MemOwner {
        MemOwner::with_mode(Mode::Policy(RefCell::new(policy)), None, LuaVersion::Lua54)
    }

    /// The handle containers keep.
    #[inline(always)]
    pub fn mem(&self) -> MemRef {
        self.0
    }

    /// The context.
    pub fn ctx(&self) -> &MemCtx {
        self.0.ctx()
    }
}

impl std::fmt::Debug for MemOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("MemOwner").field(&self.0.0).finish()
    }
}

impl Clone for MemOwner {
    fn clone(&self) -> MemOwner {
        let c = self.0.ctx();
        c.owners.set(c.owners.get() + 1);
        MemOwner(self.0)
    }
}

impl Drop for MemOwner {
    fn drop(&mut self) {
        let c = self.0.ctx();
        let n = c.owners.get() - 1;
        c.owners.set(n);
        if n == 0 {
            // SAFETY: the context came from `Box::leak` in `with_mode`, and
            // this was its last owner: every container holding a handle has
            // been dropped by now (the owners outlive them)
            drop(unsafe { Box::from_raw(self.0.0.as_ptr()) });
        }
    }
}
