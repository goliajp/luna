//! luna's containers on an allocation context, for luna's other crates.

use crate::runtime::mem::{BlockKind, LAny, LBox, LVec, MemRef, Oom};

/// An empty vector on `mem`.
///
/// # Safety
/// An owner of `mem`'s context outlives the vector and every container
/// made from it (a `Vm` owns its heap's context until it drops).
pub unsafe fn lvec_new<T>(mem: MemRef) -> LVec<T> {
    LVec::new(mem)
}

/// A vector on `mem` with room for `n` items.
///
/// # Safety
/// As [`lvec_new`]'s.
pub unsafe fn lvec_with_capacity<T>(mem: MemRef, n: usize) -> Result<LVec<T>, Oom> {
    LVec::with_capacity(mem, n)
}

/// `v` in a block of `mem`.
///
/// # Safety
/// As [`lvec_new`]'s.
pub unsafe fn lbox_new<T>(mem: MemRef, v: T) -> Result<LBox<T>, Oom> {
    LBox::new(mem, v)
}

/// `v`, of any type, in a block of `mem` counted as `kind`.
///
/// # Safety
/// As [`lvec_new`]'s.
pub unsafe fn lany_new<T: std::any::Any>(mem: MemRef, v: T, kind: BlockKind) -> Result<LAny, Oom> {
    LAny::new(mem, v, kind)
}
