//! Freed small blocks a system-allocator context keeps for its next
//! allocations of the same size.
//!
//! The collector frees short strings and other small objects by the
//! hundred each sweep step, and the program allocates the same few sizes
//! again right after. A block kept here goes back out without a trip
//! through the C allocator: a free list per size, linked through the
//! blocks themselves.
//!
//! Only blocks of 8-byte alignment and a size that is a multiple of 8 up
//! to [`MAX`] are kept, each list holding blocks of exactly one layout, so
//! a kept block can still be given back to the system with the layout it
//! was allocated with. How many are kept follows the same rule as the
//! large blocks of `pool.rs`, within the room the context gives this pool
//! out of their shared cap. Kept blocks are freed memory to the collector:
//! they count in neither the heap's bytes nor its pace, and an emergency
//! collection gives them all back before it runs.

use std::alloc::Layout;
use std::ptr::NonNull;

use super::ctx::MemCtx;

/// Largest block kept.
pub(super) const MAX: usize = 256;
const CLASSES: usize = MAX / 8;

/// Whether blocks of `layout` go through the pool.
#[inline(always)]
pub(super) fn pooled(layout: Layout) -> bool {
    layout.size() <= MAX && layout.size() % 8 == 0 && layout.align() == 8
}

/// A class's blocks: `size` bytes at 8-byte alignment.
#[inline(always)]
fn class(size: usize) -> usize {
    size / 8 - 1
}

/// What the bytes of a kept block past its link hold in builds that check
/// for a write after free.
#[cfg(any(debug_assertions, feature = "gc-verify"))]
const POISON: u8 = 0xA5;

pub(super) struct Small {
    /// each class's free list, linked through the first word of each block
    heads: [*mut u8; CLASSES],
    /// blocks on each list
    kept: [u32; CLASSES],
    /// blocks of each class allocated the last cycle: how many to keep
    want: [u32; CLASSES],
    /// blocks of each class allocated this cycle
    allocs: [u32; CLASSES],
    /// bytes of the blocks kept
    bytes: usize,
    /// at most this many bytes kept (0 until the first cycle ends)
    cap: usize,
    /// bytes allocated this cycle in pooled sizes
    allocated: usize,
}

impl Default for Small {
    fn default() -> Small {
        Small {
            heads: [std::ptr::null_mut(); CLASSES],
            kept: [0; CLASSES],
            want: [0; CLASSES],
            allocs: [0; CLASSES],
            bytes: 0,
            cap: 0,
            allocated: 0,
        }
    }
}

impl Small {
    /// A kept block of `layout` (one [`pooled`] accepts), if there is one;
    /// counts the allocation either way.
    #[inline(always)]
    pub(super) fn take(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        let (size, c) = (layout.size(), class(layout.size()));
        self.allocs[c] += 1;
        self.allocated += size;
        let p = NonNull::new(self.heads[c])?;
        // SAFETY: a kept block is a live system block of `size` bytes, at
        // least a word, whose first word links the list
        unsafe {
            #[cfg(any(debug_assertions, feature = "gc-verify"))]
            check_poison(p, size);
            self.heads[c] = p.as_ptr().cast::<*mut u8>().read();
        }
        self.kept[c] -= 1;
        self.bytes -= size;
        Some(p)
    }

    /// Keep `p`, a block of `layout` (one [`pooled`] accepts) nothing uses
    /// any more; false when the pool has no room for it, and the caller
    /// frees it.
    #[inline(always)]
    pub(super) fn keep(&mut self, p: NonNull<u8>, layout: Layout) -> bool {
        let (size, c) = (layout.size(), class(layout.size()));
        if self.kept[c] >= self.want[c] || self.bytes + size > self.cap {
            return false;
        }
        // SAFETY: the caller's contract: `p` is a block of `size` bytes, at
        // least a word, that nothing uses
        unsafe {
            #[cfg(any(debug_assertions, feature = "gc-verify"))]
            p.as_ptr().add(8).write_bytes(POISON, size - 8);
            p.as_ptr().cast::<*mut u8>().write(self.heads[c]);
        }
        self.heads[c] = p.as_ptr();
        self.kept[c] += 1;
        self.bytes += size;
        true
    }

    /// A collection cycle ended: keep for each class what the program
    /// allocated of it this cycle, within `room` bytes, the classes
    /// allocated the most first, and free the rest. Returns the bytes the
    /// pool may hold from now on.
    pub(super) fn cycle_ended(&mut self, room: usize) -> usize {
        self.cap = self.allocated.min(room);
        self.allocated = 0;
        self.want = self.allocs;
        self.allocs = [0; CLASSES];
        let mut order: [usize; CLASSES] = std::array::from_fn(|i| i);
        order.sort_unstable_by_key(|&c| std::cmp::Reverse(self.want[c]));
        let mut room = self.cap;
        for c in order {
            let size = (c + 1) * 8;
            let fit = (room / size).min(self.want[c] as usize) as u32;
            while self.kept[c] > fit {
                self.pop_free(c);
            }
            self.want[c] = fit;
            room -= fit as usize * size;
        }
        self.cap
    }

    /// Give every kept block back to the system (memory is short, or the
    /// context goes).
    pub(super) fn release(&mut self) {
        for c in 0..CLASSES {
            while self.kept[c] > 0 {
                self.pop_free(c);
            }
        }
    }

    fn pop_free(&mut self, c: usize) {
        let size = (c + 1) * 8;
        let p = self.heads[c];
        // SAFETY: the list holds `kept[c]` live system blocks of `size`
        // bytes at 8-byte alignment, linked through their first word;
        // the block leaves the list before it is freed
        unsafe {
            self.heads[c] = p.cast::<*mut u8>().read();
            std::alloc::dealloc(p, Layout::from_size_align_unchecked(size, 8));
        }
        self.kept[c] -= 1;
        self.bytes -= size;
    }
}

impl Drop for Small {
    fn drop(&mut self) {
        self.release();
    }
}

/// A kept block's bytes past its link are as `keep` left them: nothing
/// wrote to it after it was freed.
///
/// # Safety
/// `p` is a kept block of `size` bytes.
#[cfg(any(debug_assertions, feature = "gc-verify"))]
unsafe fn check_poison(p: NonNull<u8>, size: usize) {
    // SAFETY: the caller's contract
    let bytes = unsafe { std::slice::from_raw_parts(p.as_ptr().add(8), size - 8) };
    assert!(
        bytes.iter().all(|&b| b == POISON),
        "a freed block of {size} bytes at {p:p} was written after its free"
    );
}

impl MemCtx {
    /// A block of `layout` (one [`pooled`] accepts), kept or new.
    #[inline(always)]
    pub(super) fn alloc_small(&self, layout: Layout) -> Option<NonNull<u8>> {
        if let Some(p) = self.small.borrow_mut().take(layout) {
            return Some(p);
        }
        // SAFETY: the size is not 0
        NonNull::new(unsafe { std::alloc::alloc(layout) })
    }

    /// Memory is short: give the blocks kept for reuse back to the system
    /// before a collection frees more.
    pub(crate) fn release_kept(&self) {
        if matches!(self.mode, super::ctx::Mode::System) {
            self.small.borrow_mut().release();
            self.pool.borrow_mut().release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(size: usize) -> (NonNull<u8>, Layout) {
        let l = Layout::from_size_align(size, 8).unwrap();
        // SAFETY: the size is not 0
        (NonNull::new(unsafe { std::alloc::alloc(l) }).unwrap(), l)
    }

    #[test]
    fn keeps_what_the_last_cycle_allocated_and_hands_it_back() {
        let mut s = Small::default();
        let (p, l) = block(48);
        // nothing allocated yet: no room
        assert!(!s.keep(p, l));
        assert!(s.take(l).is_none());
        assert!(s.take(l).is_none());
        // the cycle allocated two blocks of 48 bytes: two are kept
        assert_eq!(s.cycle_ended(1 << 20), 96);
        assert!(s.keep(p, l));
        let (q, _) = block(48);
        assert!(s.keep(q, l));
        let (r, _) = block(48);
        assert!(!s.keep(r, l));
        // SAFETY: `r` is a block of `l` nothing uses
        unsafe { std::alloc::dealloc(r.as_ptr(), l) };
        assert_eq!(s.take(l), Some(q));
        assert_eq!(s.take(l), Some(p));
        assert!(s.take(l).is_none());
        for x in [p, q] {
            // SAFETY: the blocks came back out of the pool
            unsafe { std::alloc::dealloc(x.as_ptr(), l) };
        }
    }

    #[test]
    fn the_cap_bounds_the_bytes_kept_and_release_empties_it() {
        let mut s = Small::default();
        let l = Layout::from_size_align(64, 8).unwrap();
        for _ in 0..10 {
            assert!(s.take(l).is_none());
        }
        // room for three of the ten
        assert_eq!(s.cycle_ended(200), 200);
        let kept: Vec<_> = (0..4).map(|_| block(64).0).collect();
        let took: Vec<bool> = kept.iter().map(|&p| s.keep(p, l)).collect();
        assert_eq!(took, [true, true, true, false]);
        // SAFETY: the one the pool refused is a block of `l` nothing uses
        unsafe { std::alloc::dealloc(kept[3].as_ptr(), l) };
        s.release();
        assert_eq!((s.bytes, s.kept[class(64)]), (0, 0));
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "gc-verify"))]
    #[should_panic(expected = "written after its free")]
    fn a_write_after_free_is_caught_when_the_block_comes_back() {
        let mut s = Small::default();
        let (p, l) = block(32);
        assert!(s.take(l).is_none());
        s.cycle_ended(1 << 20);
        assert!(s.keep(p, l));
        // SAFETY: the block is still allocated, held by the pool
        unsafe { p.as_ptr().add(20).write(1) };
        let _ = s.take(l);
    }
}
