//! The blocks natives are made in.

use super::*;
use crate::runtime::mem::Oom;

impl Heap {
    /// A free slot for a native: one a freed native left, or the next of
    /// the newest slab block, or of a new one.
    pub(in crate::runtime::heap) fn slab_block(&mut self) -> Result<*mut NativeClosure, Oom> {
        let s = &mut self.slab;
        if let Some(p) = s.free.pop() {
            return Ok(p.as_ptr());
        }
        if s.used == SLAB_LEN || s.blocks.is_empty() {
            let p = self
                .mem()
                .ctx()
                .alloc(slab_layout(), crate::runtime::mem::BlockKind::Other)
                .ok_or(Oom(self.mem()))?;
            let s = &mut self.slab;
            s.blocks.push(p.cast());
            s.used = 0;
        }
        let s = &mut self.slab;
        let block = *s.blocks.last().expect("a block was made above");
        // SAFETY: `used` is below the block's length
        let p = unsafe { block.as_ptr().add(s.used) };
        s.used += 1;
        Ok(p)
    }

    /// The slot of `p`, a native of the slab dropped already, free again.
    pub(in crate::runtime::heap) fn slab_free(&mut self, p: *mut NativeClosure) {
        if !self.dropping {
            self.slab
                .free
                .push(std::ptr::NonNull::new(p).expect("a native's slot"));
        }
    }

    /// Give the slab blocks back, the natives in them dropped already.
    pub(in crate::runtime::heap) fn free_slab(&mut self) {
        for b in std::mem::take(&mut self.slab.blocks) {
            // SAFETY: each block came from this context with this layout
            unsafe { self.mem().ctx().free(b.cast(), slab_layout()) };
        }
    }
}

/// Natives in a slab block.
const SLAB_LEN: usize = 64;

fn slab_layout() -> std::alloc::Layout {
    std::alloc::Layout::array::<NativeClosure>(SLAB_LEN).expect("a slab block's size")
}

/// Blocks the natives are made in, `SLAB_LEN` each: one allocation for
/// many of them (most are library functions, which live as long as the
/// state), the slots of freed ones made again, the blocks given back
/// with the heap.
#[derive(Default)]
pub(crate) struct NativeSlab {
    blocks: Vec<std::ptr::NonNull<NativeClosure>>,
    /// slots taken in the newest block
    used: usize,
    /// slots freed natives left
    free: Vec<std::ptr::NonNull<NativeClosure>>,
}
