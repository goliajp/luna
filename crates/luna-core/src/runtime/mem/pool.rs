//! Freed blocks a system-allocator context keeps for its next allocations
//! of the same layout.
//!
//! Once the process has a second thread (the LLVM backend's compile
//! thread, or the host's own), glibc takes its arena lock for every block
//! past its per-thread cache (1032 bytes). The collector frees the blocks
//! of dead tables in bursts, one sweep at a time, and the program then
//! allocates blocks of the same sizes again until the next sweep: keeping
//! those blocks here hands them back without any lock.
//!
//! How many of a layout are kept follows the program: as many as it
//! allocated of that layout during the last collection cycle, and in all
//! at most the bytes it allocated of pooled sizes that cycle, capped by
//! the larger of [`CAP_FLOOR`] and an eighth of the live heap, and by
//! [`CAP_MAX`]. The pool is trimmed to that at the end of every cycle, and
//! gives everything back when its context goes. Kept blocks are freed
//! memory to the collector: they count in neither the heap's bytes nor
//! its pace.

use std::alloc::Layout;
use std::ptr::NonNull;

use super::ctx::{BlockKind, MemCtx, Mode};

/// Smallest block kept: smaller ones are in glibc's per-thread cache.
pub(super) const MIN: usize = 1025;
/// Largest block kept.
pub(super) const MAX: usize = 256 * 1024;
/// Layouts tracked at a time.
const LAYOUTS: usize = 32;
/// The pool holds at most what the program allocated of pooled sizes the
/// last cycle, and at most the larger of [`CAP_FLOOR`] and the live heap
/// divided by [`CAP_SHARE`], and never more than [`CAP_MAX`].
const CAP_SHARE: usize = 8;
const CAP_FLOOR: usize = 1024 * 1024;
const CAP_MAX: usize = 8 * 1024 * 1024;

/// Whether blocks of `size` bytes go through the pool.
#[inline(always)]
pub(super) fn pooled(size: usize) -> bool {
    (MIN..=MAX).contains(&size)
}

struct List {
    layout: Layout,
    blocks: Vec<NonNull<u8>>,
    /// blocks of this layout allocated this cycle
    allocs: u32,
    /// blocks of this layout allocated the last cycle: how many to keep
    want: u32,
}

#[derive(Default)]
pub(super) struct Pool {
    lists: Vec<List>,
    /// bytes of the blocks kept
    bytes: usize,
    /// at most this many bytes kept (0 until the first cycle ends)
    cap: usize,
    /// bytes of pooled sizes allocated this cycle
    allocated: usize,
}

impl Pool {
    fn index(&mut self, layout: Layout) -> Option<usize> {
        if let Some(i) = self.lists.iter().position(|l| l.layout == layout) {
            return Some(i);
        }
        if self.lists.len() == LAYOUTS {
            return None;
        }
        self.lists.push(List {
            layout,
            blocks: Vec::new(),
            allocs: 0,
            want: 0,
        });
        Some(self.lists.len() - 1)
    }

    /// A kept block of exactly `layout`, if there is one; counts the
    /// allocation either way.
    pub(super) fn take(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        let i = self.index(layout)?;
        let l = &mut self.lists[i];
        l.allocs += 1;
        self.allocated += layout.size();
        let p = l.blocks.pop()?;
        self.bytes -= layout.size();
        Some(p)
    }

    /// Keep `p`, a block of `layout` from the system allocator nothing
    /// uses any more; false when the pool has no room for it, and the
    /// caller frees it.
    pub(super) fn keep(&mut self, p: NonNull<u8>, layout: Layout) -> bool {
        if self.bytes + layout.size() > self.cap {
            return false;
        }
        let Some(i) = self.lists.iter().position(|l| l.layout == layout) else {
            return false;
        };
        let l = &mut self.lists[i];
        if l.blocks.len() >= l.want as usize {
            return false;
        }
        l.blocks.push(p);
        self.bytes += layout.size();
        true
    }

    /// A collection cycle ended with `live` bytes in the heap: keep for
    /// each layout what the program allocated of it this cycle, within
    /// the share of `live`, and free the rest.
    pub(super) fn cycle_ended(&mut self, live: usize) {
        let share = (live / CAP_SHARE).clamp(CAP_FLOOR, CAP_MAX);
        self.cap = self.allocated.min(share);
        self.allocated = 0;
        for l in &mut self.lists {
            l.want = l.allocs;
            l.allocs = 0;
        }
        // layouts allocated the most keep their blocks first
        self.lists.sort_by_key(|l| std::cmp::Reverse(l.want));
        let mut room = self.cap;
        let mut bytes = 0;
        for l in &mut self.lists {
            let fit = (room / l.layout.size()).min(l.want as usize);
            for p in l.blocks.drain(fit.min(l.blocks.len())..) {
                // SAFETY: a kept block came from the system allocator with
                // this layout and nothing uses it
                unsafe { std::alloc::dealloc(p.as_ptr(), l.layout) };
            }
            l.want = fit as u32;
            room -= l.want as usize * l.layout.size();
            bytes += l.blocks.len() * l.layout.size();
        }
        self.bytes = bytes;
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        for l in &self.lists {
            for p in &l.blocks {
                // SAFETY: as in `cycle_ended`
                unsafe { std::alloc::dealloc(p.as_ptr(), l.layout) };
            }
        }
    }
}

impl MemCtx {
    /// A block of `layout` (of a size [`pooled`]), kept or new.
    #[inline(never)]
    pub(super) fn alloc_pooled(&self, layout: Layout) -> Option<NonNull<u8>> {
        if let Some(p) = self.pool.borrow_mut().take(layout) {
            return Some(p);
        }
        // SAFETY: the size is not 0
        NonNull::new(unsafe { std::alloc::alloc(layout) })
    }

    /// [`MemCtx::realloc`] in `Mode::System` where the old or the new size
    /// is [`pooled`]: a block from [`MemCtx::alloc`], the bytes copied, the
    /// old block given to [`MemCtx::free`].
    ///
    /// # Safety
    /// As for [`MemCtx::realloc`].
    #[inline(never)]
    pub(super) unsafe fn move_pooled(
        &self,
        p: NonNull<u8>,
        layout: Layout,
        new: usize,
    ) -> Option<NonNull<u8>> {
        // SAFETY: `new` is not 0 and fits a layout of `layout`'s alignment
        let to = unsafe { Layout::from_size_align_unchecked(new, layout.align()) };
        let q = self.alloc(to, BlockKind::Other)?;
        // SAFETY: both blocks hold at least the bytes copied and do not
        // overlap; `p` is a live block of `layout` from this context
        unsafe {
            std::ptr::copy_nonoverlapping(p.as_ptr(), q.as_ptr(), layout.size().min(new));
            self.free(p, layout);
        }
        Some(q)
    }

    /// The collector finished a cycle with `live` bytes in the heap.
    pub(crate) fn gc_cycle_ended(&self, live: usize) {
        if matches!(self.mode, Mode::System) {
            self.pool.borrow_mut().cycle_ended(live);
        }
    }
}
