//! Memory a Vm allocates: the allocation context ([`MemCtx`]) every block
//! comes from, and the containers that hold such blocks ([`LVec`],
//! [`LBox`], [`LSlice`]).
//!
//! The standard library's `Vec` and `Box` always use the global allocator,
//! so a Vm whose host supplies the memory (`lua_newstate`'s `lua_Alloc`,
//! or a [`MemoryPolicy`]) keeps its data in these instead. Each container
//! carries a [`MemRef`] to free and grow its block, and every growth can
//! fail with [`Oom`], leaving the container unchanged.

mod any;
mod boxed;
mod ctx;
mod map;
mod vec;

pub use any::LAny;
pub use boxed::{LBox, LSlice};
pub use ctx::{BlockKind, MemCtx, MemOwner, MemRef, MemoryLimit, MemoryPolicy, Oom, RawAllocFn};
pub use map::{LMap, WordHasher, word_hash};
pub use vec::LVec;

#[cfg(test)]
mod tests;

/// End the process for an allocation of `layout` that failed, as the
/// standard library's containers do.
#[cold]
pub(crate) fn oom_abort(layout: std::alloc::Layout) -> ! {
    std::alloc::handle_alloc_error(layout)
}
