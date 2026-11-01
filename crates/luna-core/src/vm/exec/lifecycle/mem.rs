//! Making a Vm whose memory comes from a host function or a policy.

use super::*;

impl Vm {
    /// [`Vm::new_minimal`] whose memory comes from `mem`: the system
    /// allocator watched by a [`crate::runtime::mem::MemoryPolicy`]
    /// ([`crate::runtime::mem::MemOwner::policy`]), or a host allocation
    /// function ([`crate::runtime::mem::MemOwner::raw`]).
    pub fn new_minimal_with_mem(version: LuaVersion, mem: MemOwner) -> Vm {
        Vm::new_minimal_on(version, Heap::new_on(mem))
    }

    /// [`Vm::new_minimal_with_mem`], or `None` when the allocation context
    /// cannot give the Vm the memory it starts with (PUC `lua_newstate`
    /// returns NULL then); what was allocated is freed.
    pub fn try_new_minimal_with_mem(version: LuaVersion, mem: MemOwner) -> Option<Vm> {
        let heap = Heap::try_with_mem(mem, crate::runtime::heap::fresh_seed())?;
        Vm::try_new_minimal_on(version, heap)
    }

    /// [`Vm::try_new_minimal_with_mem`] with string hashing seeded by `seed`
    /// rather than a seed of its own, as [`Vm::new_minimal_with_hash_seed`]:
    /// with an allocator that behaves the same each run, two runs of a
    /// program lay out their tables, and so iterate them, the same way.
    /// `None` when `mem` cannot give the Vm the memory it starts with.
    pub fn try_new_minimal_with_mem_and_seed(
        version: LuaVersion,
        mem: MemOwner,
        seed: u32,
    ) -> Option<Vm> {
        Vm::try_new_minimal_on(version, Heap::try_with_mem(mem, seed)?)
    }

    /// [`Vm::new`] whose memory comes from `mem` (see
    /// [`Vm::new_minimal_with_mem`]).
    pub fn new_with_mem(version: LuaVersion, mem: MemOwner) -> Vm {
        let mut vm = Vm::new_minimal_with_mem(version, mem);
        crate::vm::lib_mem::opened(vm.open_all_libs());
        vm
    }

    /// Bytes the Vm has allocated and not freed, as its allocation
    /// context counts them: `None` on the system allocator, which is not
    /// counted. Vms given clones of one `MemOwner` share one count, the
    /// bytes of them all.
    pub fn memory_in_use(&self) -> Option<usize> {
        self.heap.mem().ctx().in_use()
    }
}
