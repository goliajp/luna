//! Resizing a block from the system allocator.

use std::alloc::Layout;
use std::ptr::NonNull;

/// Blocks up to this size are resized by moving them to a fresh block:
/// glibc keeps small blocks (up to 1032 bytes) in a cache of each thread,
/// which `malloc` and `free` use without a lock, while its `realloc` takes
/// the arena lock as soon as the process has a second thread (the LLVM
/// backend's compile thread, or the host's own).
pub(super) const SMALL_MOVE: usize = 1024;

/// [`MemCtx::realloc`] for a block of at most [`SMALL_MOVE`] bytes from
/// the system allocator.
///
/// # Safety
/// As for [`MemCtx::realloc`].
#[inline(never)]
pub(super) unsafe fn move_small(p: NonNull<u8>, layout: Layout, new: usize) -> Option<NonNull<u8>> {
    // SAFETY: `new` is not 0 and fits a layout of `layout`'s alignment;
    // both blocks hold at least the bytes copied and do not overlap; `p`
    // came from the system allocator with `layout`
    unsafe {
        let q = NonNull::new(std::alloc::alloc(Layout::from_size_align_unchecked(
            new,
            layout.align(),
        )))?;
        std::ptr::copy_nonoverlapping(p.as_ptr(), q.as_ptr(), layout.size().min(new));
        std::alloc::dealloc(p.as_ptr(), layout);
        Some(q)
    }
}
