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
mod vec_abort;

pub use any::LAny;
pub use boxed::{LBox, LSlice};
pub use ctx::{BlockKind, MemCtx, MemOwner, MemRef, MemoryLimit, MemoryPolicy, Oom, RawAllocFn};
pub use map::{LMap, WordHasher, word_hash};
pub use vec::{Drain, LVec};

#[cfg(test)]
mod tests;

/// An allocation of `layout` failed where no memory error can be
/// returned: inside a load (see [`catch_load_oom`]) the load unwinds to its
/// entry, which raises the memory error; anywhere else the process ends, as
/// the standard library's containers do.
#[cold]
pub(crate) fn oom_abort(layout: std::alloc::Layout) -> ! {
    if LOAD_UNWINDS.with(std::cell::Cell::get) {
        std::panic::resume_unwind(Box::new(LoadOom));
    }
    std::alloc::handle_alloc_error(layout)
}

/// What a load unwinds with when it runs out of memory. Private: only
/// [`catch_load_oom`] stops it.
struct LoadOom;

std::thread_local! {
    /// set while the frontend of a load runs on this thread
    static LOAD_UNWINDS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Restores [`LOAD_UNWINDS`] when it goes out of scope, unwinding or not.
struct LoadUnwinds(bool);

impl LoadUnwinds {
    fn set(on: bool) -> LoadUnwinds {
        LoadUnwinds(LOAD_UNWINDS.with(|c| c.replace(on)))
    }
}

impl Drop for LoadUnwinds {
    fn drop(&mut self) {
        LOAD_UNWINDS.with(|c| c.set(self.0));
    }
}

/// Run `f`, the frontend of a load (lexer, parser, compiler), so that an
/// allocation failing inside it unwinds back here: `Err(())` then, with
/// every block `f` held dropped (given back to the context) on the way.
/// Any other panic continues unwinding. With `panic = "abort"` a failed
/// allocation ends the process instead.
pub(crate) fn catch_load_oom<R>(f: impl FnOnce() -> R) -> Result<R, ()> {
    let _scope = LoadUnwinds::set(true);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(p) if p.is::<LoadOom>() => Err(()),
        Err(p) => std::panic::resume_unwind(p),
    }
}

/// Run `f` with a failed allocation ending the process again, as outside a
/// load: for the reader callbacks a streamed load makes, which run Lua code.
pub(crate) fn outside_load<R>(f: impl FnOnce() -> R) -> R {
    let _scope = LoadUnwinds::set(false);
    f()
}

/// Whether `p`, a panic caught elsewhere, is a load's memory failure, which
/// must keep unwinding to its load.
pub(crate) fn is_load_oom(p: &(dyn std::any::Any + Send)) -> bool {
    p.is::<LoadOom>()
}
