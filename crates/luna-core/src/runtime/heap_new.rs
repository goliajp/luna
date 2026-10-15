//! Making a heap, and the allocation context it takes its memory from.

use super::*;
use crate::runtime::mem::{LVec, Oom, oom_abort};
use std::alloc::Layout;

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
            refinalize: true,
            sizes: crate::runtime::puc_size::PucSizes::of(crate::runtime::table::Dialect::L55),
            puc: Default::default(),
            #[cfg(feature = "gc-trace")]
            trace: Default::default(),
            signed_zero_keys: false,
            table_dialect: crate::runtime::table::Dialect::L55,
            hash51: false,
            defer_thread_cycle_finalize: false,
            chunk_roots: Vec::new(),
            weak_watch: Vec::new(),
            threads: Vec::new(),
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

impl Drop for Heap {
    fn drop(&mut self) {
        // free everything regardless of reachability, including any list still
        // detached for an in-flight incremental sweep
        // SAFETY: the heap is being dropped, so nothing uses its objects any more; `all`, `sweep_cur` and `fixed` hold every object it allocated and has not freed, each exactly once, and `next` is read before `free_obj`; the pool holds tables from `alloc_block` already unlinked from every list
        unsafe {
            for mut cur in [self.all, self.sweep_cur, self.fixed] {
                while !cur.is_null() {
                    let next = (*cur).next;
                    self.free_obj(cur);
                    cur = next;
                }
            }
            // the pooled tables' interiors were freed when they were
            // recycled; only their blocks are left
            for &ptr in self.table_pool.take().iter() {
                self.free_block(ptr.as_ptr());
            }
        }
    }
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

impl Heap {
    /// Create a fresh suspended coroutine wrapping `body`. The new thread
    /// inherits the creator's globals table; a `setfenv(0, env)` inside it
    /// will retune that copy without affecting the creator.
    pub fn new_coro(
        &mut self,
        body: Value,
        globals: Gc<crate::runtime::Table>,
    ) -> Gc<crate::runtime::Coro> {
        self.try_new_coro(body, globals)
            .unwrap_or_else(|_| oom_abort(Layout::new::<crate::runtime::Coro>()))
    }

    /// [`Heap::new_coro`], failing when memory runs out.
    pub fn try_new_coro(
        &mut self,
        body: Value,
        globals: Gc<crate::runtime::Table>,
    ) -> Result<Gc<crate::runtime::Coro>, Oom> {
        let mem = self.mem();
        let size = self.sizes.thread;
        let l51 = self.table_dialect == table::Dialect::L51;
        // `stack_init`: `BASIC_STACK_SIZE`, 5.1 with its extra slots
        let stack_size = if l51 { 45 } else { 40 };
        let coro = crate::runtime::Coro {
            hdr: GcHeader::new(ObjTag::Coro),
            status: crate::runtime::CoroStatus::Suspended,
            body,
            started: false,
            resumer: None,
            resume_at: None,
            error_value: None,
            error_status: crate::runtime::coroutine::ErrorStatus::Run,
            error_traceback: None,
            error_levels: None,
            natives: 0..0,
            stack: LVec::new(mem),
            frames: LVec::new(mem),
            open_upvals: LVec::new(mem),
            tbc: LVec::new(mem),
            top: 0,
            meta_conts: 0,
            stale_frames: 0,
            stack_size,
            frame_size: if l51 {
                acct::BASIC_CI_51
            } else {
                u32::MAX
            },
            ci_alloc: if l51 { u32::MAX } else { 0 },
            hook: crate::vm::exec::HookState::default(),
            globals,
            host_stack: LVec::new(mem),
            host_state: None,
        };
        let co = self.adopt_counted(coro, PucKind::Thread, size)?;
        if self.puc.silent == 0 {
            self.threads.push(co);
        }
        self.acct_thread_blocks(stack_size);
        Ok(co)
    }
}
