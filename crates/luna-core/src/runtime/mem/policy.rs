//! A Rust host's view of a Vm's memory: [`MemoryPolicy`].

use super::BlockKind;

/// A safe way for a Rust host to watch and limit a Vm's memory. The
/// memory itself still comes from the system allocator.
pub trait MemoryPolicy {
    /// A block is about to grow from `old` bytes to `new` (`old` is 0 for
    /// a new block); `in_use` is what the Vm has allocated so far. Return
    /// false to make the allocation fail.
    fn allow(&mut self, old: usize, new: usize, kind: BlockKind, in_use: usize) -> bool;
    /// `size` bytes were given back (a block freed, or the part a block
    /// shrank by).
    fn freed(&mut self, size: usize) {
        let _ = size;
    }
}

/// A [`MemoryPolicy`] that refuses any growth past `limit` bytes in use.
#[derive(Clone, Copy, Debug)]
pub struct MemoryLimit(pub usize);

impl MemoryPolicy for MemoryLimit {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, in_use: usize) -> bool {
        in_use - old.min(in_use) + new <= self.0
    }
}
