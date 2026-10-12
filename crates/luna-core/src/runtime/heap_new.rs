//! Making a heap, and the allocation context it takes its memory from.

use super::*;

impl Heap {
    /// Build a fresh empty heap with default GC pacing and no memory cap.
    pub fn new() -> Heap {
        Heap::with_seed(make_seed())
    }

    /// Hash the interned strings again with `seed` and the dialect's
    /// function.
    pub(crate) fn rehash_strings(&mut self, seed: u32) {
        let strict = self.table_dialect <= table::Dialect::L53;
        self.strings.rehash(seed, self.hash51, strict);
    }

    /// The seed strings are hashed with.
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// [`Heap::new`] hashing strings with `seed`.
    pub fn with_seed(seed: u32) -> Heap {
        Heap::with_mem(crate::runtime::mem::MemOwner::system(), seed)
    }

    /// A heap whose memory comes from `mem`, hashing strings with `seed`.
    pub fn with_mem(mem: crate::runtime::mem::MemOwner, seed: u32) -> Heap {
        Heap::try_with_mem(mem, seed).unwrap_or_else(|| oom_abort_heap())
    }

    /// [`Heap::with_mem`], or `None` when the allocation context cannot
    /// give the heap its first blocks.
    pub fn try_with_mem(mem: crate::runtime::mem::MemOwner, seed: u32) -> Option<Heap> {
        let strings = StringTable::new(mem.mem()).ok()?;
        let mut h = Heap {
            all: ptr::null_mut(),
            fixed: ptr::null_mut(),
            fix_natives: false,
            strings,
            seed,
            live: 0,
            bytes: 0,
            next_gc: GC_MIN_THRESHOLD,
            gc_limit: GC_MIN_THRESHOLD,
            current_white: WHITE0,
            gray: crate::runtime::mem::LVec::new(mem.mem()),
            gray_overflow: false,
            young: ptr::null_mut(),
            store_failed: false,
            propagate: None,
            phase: GcPhase::Pause,
            sweep_cur: ptr::null_mut(),
            gc_stopped: false,
            finalize: crate::runtime::mem::LVec::new(mem.mem()),
            tobefnz: crate::runtime::mem::LVec::new(mem.mem()),
            no_ephemeron: false,
            signed_zero_keys: false,
            table_dialect: crate::runtime::table::Dialect::L55,
            hash51: false,
            defer_thread_cycle_finalize: false,
            chunk_roots: Vec::new(),
            track_chunk_roots: false,
            mem_cap: None,
            table_pool: crate::runtime::mem::LVec::new(mem.mem()),
            #[cfg(feature = "gc-verify")]
            recently_freed: std::collections::HashSet::new(),
            mem,
        };
        h.make_memerr().ok()?;
        Some(h)
    }

    /// The handle the heap's containers allocate through.
    #[inline(always)]
    pub fn mem(&self) -> crate::runtime::mem::MemRef {
        self.mem.mem()
    }

    /// An owner of the heap's allocation context, for holders that must
    /// keep it alive as long as they do (the Vm's own containers).
    pub fn mem_owner(&self) -> crate::runtime::mem::MemOwner {
        self.mem.clone()
    }
}
