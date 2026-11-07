//! A Rust host's view of a Vm's memory: [`MemoryPolicy`].

use super::BlockKind;

/// Declares the trait, `Send` with the `send` feature: the Vms sharing a
/// budget may run on other threads.
macro_rules! policy {
    ($($bound:tt)*) => {
        /// A safe way for a Rust host to watch and limit a Vm's memory. The
        /// memory itself still comes from the system allocator. With the
        /// `send` feature a policy is `Send`: a Vm sent to another thread
        /// takes it along, and Vms sharing it may run on several.
        ///
        /// What the policy sees is what the Vm allocates to run Lua: values,
        /// stacks, frames and the buffers compiled code runs with. The JIT
        /// compilers' own memory is not: the data a trace or function is
        /// compiled through (freed once it is compiled) and the machine
        /// code made from it come from the system allocator and the
        /// operating system's pages, and do not grow with what the Lua
        /// program allocates. They grow with the code that gets hot: a
        /// trace records at most
        /// [`MAX_TRACE_LEN`](crate::jit::trace::MAX_TRACE_LEN)
        /// instructions, calls inlined at most
        /// [`MAX_INLINE_DEPTH`](crate::jit::trace::MAX_INLINE_DEPTH) deep;
        /// a loop head or function entry gets one trace (after at most a
        /// few failed attempts), compiled again once for the optimizing
        /// tier; each exit of a
        /// trace gets at most one side trace, itself a trace; a function is
        /// compiled whole once. A host that must bound every byte the Vm
        /// takes turns the JIT off.
        ///
        /// The requests the policy sees, their sizes and their order, are
        /// how this version of the Vm happens to allocate: they change with
        /// the JIT on or off (compiled code keeps in machine registers and
        /// native frames what the interpreter keeps in its stacks) and from
        /// one version to the next, and are not part of the compatibility
        /// promise. A program a policy makes fail one way may run through
        /// with the JIT on, or fail elsewhere. A host that needs a limit
        /// with the same outcome either way sets it with
        /// [`Vm::set_memory_cap`](crate::vm::Vm::set_memory_cap), under
        /// which no compiled code runs.
        pub trait MemoryPolicy $($bound)* {
            /// A block is about to grow from `old` bytes to `new` (`old` is
            /// 0 for a new block); `in_use` is what the Vm has allocated so
            /// far. Return false to make the allocation fail.
            fn allow(&mut self, old: usize, new: usize, kind: BlockKind, in_use: usize) -> bool;
            /// `size` bytes were given back (a block freed, or the part a
            /// block shrank by).
            fn freed(&mut self, size: usize) {
                let _ = size;
            }
        }
    };
}

#[cfg(not(feature = "send"))]
policy!();
#[cfg(feature = "send")]
policy!(: Send);

/// A [`MemoryPolicy`] that refuses any growth past `limit` bytes in use.
#[derive(Clone, Copy, Debug)]
pub struct MemoryLimit(pub usize);

impl MemoryPolicy for MemoryLimit {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, in_use: usize) -> bool {
        in_use - old.min(in_use) + new <= self.0
    }
}
