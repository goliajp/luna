//! The block a state's first small objects are cut from.
//!
//! Opening the libraries makes a few hundred small objects (the library
//! functions' names, their tables' parts), nearly all of which live as
//! long as the state. With the system allocator each would be a call to
//! `malloc` and, when the state goes, to `free`. Instead the first
//! [`ARENA`] bytes of small objects are cut one after the other from one
//! block, taken with the context and given back with it. A piece that is
//! freed is not reused: few are, and the block is small.

use std::alloc::Layout;
use std::cell::Cell;
use std::ptr::NonNull;

/// The block and how much of it is used: `[start, end)`, cut up to `next`;
/// all 0 without one.
#[derive(Default)]
pub(super) struct Arena {
    start: Cell<usize>,
    end: Cell<usize>,
    next: Cell<usize>,
}

/// The alignment of the block, and of every piece cut from it.
const ALIGN: usize = 8;

/// The block's size: what opening the standard libraries cuts, a little
/// over 16000 bytes in every dialect on 64-bit targets.
pub(super) const ARENA: usize = 16 * 1024;

impl Arena {
    /// Take the block and cut what follows from it.
    pub(super) fn open(&self) {
        let bytes = ARENA;
        let layout = Layout::from_size_align(bytes, ALIGN).expect("an arena's size");
        // SAFETY: the size is not 0
        let Some(p) = NonNull::new(unsafe { std::alloc::alloc(layout) }) else {
            return;
        };
        let p = p.as_ptr() as usize;
        self.start.set(p);
        self.next.set(p);
        self.end.set(p + bytes);
    }

    /// A piece of `layout` (8-byte aligned, a multiple of 8 bytes) cut from
    /// the block, when it has room.
    #[inline(always)]
    pub(super) fn cut(&self, layout: Layout) -> Option<NonNull<u8>> {
        let at = self.next.get();
        let end = at + layout.size();
        if end > self.end.get() {
            return None;
        }
        self.next.set(end);
        NonNull::new(at as *mut u8)
    }

    /// Whether `p` was cut from the block.
    #[inline(always)]
    pub(super) fn holds(&self, p: NonNull<u8>) -> bool {
        let a = p.as_ptr() as usize;
        a >= self.start.get() && a < self.end.get()
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let (start, end) = (self.start.get(), self.end.get());
        if start != 0 {
            let layout = Layout::from_size_align(end - start, ALIGN).expect("an arena's size");
            // SAFETY: the block came from `open` with this layout, and the
            // context, whose pieces are all that point into it, goes now
            unsafe { std::alloc::dealloc(start as *mut u8, layout) };
        }
    }
}

impl super::MemCtx {
    /// [`MemCtx::realloc`] of `p`, cut from the block: moved to a new
    /// block, the piece left behind.
    ///
    /// # Safety
    /// As for [`MemCtx::realloc`].
    #[inline(never)]
    pub(super) unsafe fn move_from_arena(
        &self,
        p: NonNull<u8>,
        layout: Layout,
        new: usize,
    ) -> Option<NonNull<u8>> {
        let l = Layout::from_size_align(new, layout.align()).ok()?;
        let q = self.alloc(l, super::BlockKind::Other)?;
        // SAFETY: both blocks hold the bytes copied and do not overlap
        unsafe { std::ptr::copy_nonoverlapping(p.as_ptr(), q.as_ptr(), layout.size().min(new)) };
        Some(q)
    }
}
