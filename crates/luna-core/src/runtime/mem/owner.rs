//! The owners of a [`MemCtx`]: the context lives until the last one goes.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use super::MemoryPolicy;
use super::budget::Budget;
use super::ctx::{MemCtx, MemRef, Mode, RawAllocFn};
use super::kind::kind_codes;
use crate::version::LuaVersion;

/// One owner of a [`MemCtx`]; the context is freed with its last owner.
///
/// Clones can be given to several Vms: they then share one budget (the
/// host function or the policy sees every Vm's blocks, and
/// [`MemCtx::in_use`] counts them all), while each Vm's heap allocates
/// through a context of its own.
///
/// Only luna makes containers on a context, so none can outlive its last
/// owner; the example compiles, the one after it fails to.
///
/// ```
/// use luna_core::runtime::mem::MemOwner;
/// let o = MemOwner::system();
/// assert_eq!(o.clone().ctx().in_use(), None);
/// ```
///
/// ```compile_fail,E0624
/// use luna_core::runtime::mem::{LVec, MemOwner};
/// let o = MemOwner::system();
/// let v: LVec<u64> = LVec::new(o.mem()); // error[E0624]: `new` and `mem` are private
/// ```
pub struct MemOwner(MemRef);

impl MemOwner {
    fn with_mode(
        mode: Mode,
        raw: Option<(RawAllocFn, *mut c_void)>,
        policy: Option<Box<dyn MemoryPolicy>>,
        v: LuaVersion,
    ) -> MemOwner {
        let budget = Box::new(Budget {
            raw: Cell::new(raw),
            policy: policy.map(RefCell::new),
            total: Cell::new(0),
            users: Cell::new(0),
        });
        MemOwner::on(mode, NonNull::from(Box::leak(budget)), kind_codes(v))
    }

    /// A new context on `budget`, a live budget it takes one use of.
    fn on(mode: Mode, budget: NonNull<Budget>, codes: [u8; super::ctx::KINDS]) -> MemOwner {
        let ctx = Box::new(MemCtx {
            mode,
            budget,
            codes,
            owners: Cell::new(1),
            claimed: Cell::new(false),
            memerr: Cell::new(std::ptr::null_mut()),
            oom_raised: Cell::new(0),
            pool: RefCell::default(),
            small: RefCell::default(),
            arena: Default::default(),
        });
        // the system allocator's first small objects come from one block
        if matches!(ctx.mode, Mode::System) {
            ctx.arena.open();
        }
        let b = ctx.budget();
        b.users.set(b.users.get() + 1);
        MemOwner(MemRef(NonNull::from(Box::leak(ctx))))
    }

    /// A context on the system allocator.
    pub fn system() -> MemOwner {
        MemOwner::with_mode(Mode::System, None, None, LuaVersion::Lua54)
    }

    /// A context whose memory comes from `f` with `ud`, numbering the
    /// kinds of new blocks as dialect `v` does.
    ///
    /// # Safety
    /// `f` follows PUC's `lua_Alloc` contract and may be called with
    /// `ud` until the last owner of this context, and of every context a
    /// Vm given a clone of it makes, is dropped; those drops free through
    /// it every block still allocated.
    pub unsafe fn raw(f: RawAllocFn, ud: *mut c_void, v: LuaVersion) -> MemOwner {
        MemOwner::with_mode(Mode::Raw, Some((f, ud)), None, v)
    }

    /// A context on the system allocator that asks `policy` first.
    pub fn policy(policy: Box<dyn MemoryPolicy>) -> MemOwner {
        MemOwner::with_mode(Mode::Policy, None, Some(policy), LuaVersion::Lua54)
    }

    /// The handle containers keep.
    #[inline(always)]
    pub(crate) fn mem(&self) -> MemRef {
        self.0
    }

    /// The context.
    pub fn ctx(&self) -> &MemCtx {
        self.0.ctx()
    }

    /// The owner of the context a new heap allocates through: this one,
    /// unless another heap has taken it, else a new context on the same
    /// budget. A context's memory error string and kept blocks are its
    /// one heap's.
    pub(crate) fn for_heap(self) -> MemOwner {
        let c = self.0.ctx();
        if !c.claimed.get() {
            c.claimed.set(true);
            return self;
        }
        let own = MemOwner::on(c.mode, c.budget, c.codes);
        own.0.ctx().claimed.set(true);
        own
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
            // the budget outlives the context, which holds a use of it
            let (budget, b) = (c.budget, c.budget());
            // SAFETY: the context came from `Box::leak` in `on`, and this
            // was its last owner: every container holding a handle has been
            // dropped by now (the owners outlive them)
            drop(unsafe { Box::from_raw(self.0.0.as_ptr()) });
            let users = b.users.get() - 1;
            b.users.set(users);
            if users == 0 {
                // SAFETY: the budget came from `Box::leak` in `with_mode`,
                // and no context uses it any more
                drop(unsafe { Box::from_raw(budget.as_ptr()) });
            }
        }
    }
}
