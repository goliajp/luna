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
            pace: crate::runtime::gc_pace::Pace::new(),
            gc_limit: usize::MAX,
            current_white: WHITE0,
            gray: mark::GrayStack::default(),
            grayagain: mark::GrayStack::default(),
            strt: Default::default(),
            gray_overflow: false,
            young: ptr::null_mut(),
            store_failed: false,
            propagate: None,
            phase: GcPhase::Pause,
            sweep_prev: ptr::null_mut(),
            gc_stopped: false,
            gc_fin_stop: false,
            dropping: false,
            finalize: crate::runtime::mem::LVec::new(mem.mem()),
            tobefnz: crate::runtime::mem::LVec::new(mem.mem()),
            tobefnz_head: 0,
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
            gc_event_51: None,
            chunk_roots: Vec::new(),
            weak_watch: Vec::new(),
            weak_objs: Vec::new(),
            threads: Vec::new(),
            open_dead: Default::default(),
            track_chunk_roots: false,
            mem_cap: None,
            table_pool: Vec::new(),
            slab: Default::default(),
            env51_box: None,
            stdin_ud: ptr::null_mut(),
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
        self.dropping = true;
        // nothing is counted any more
        self.puc.silent += 1;
        // every block goes straight back to the system
        self.mem().ctx().close_pools();
        // free everything regardless of reachability, including any list still
        // detached for an in-flight incremental sweep
        // SAFETY: the heap is being dropped, so nothing uses its objects any more; `all`, `fixed` and the held strings hold every object it allocated and has not freed, each exactly once, and `next` is read before `free_obj`; the pool holds tables from `alloc_block` already unlinked from every list
        unsafe {
            for mut cur in [self.all, self.fixed] {
                while !cur.is_null() {
                    let next = (*cur).next;
                    self.free_obj_closing(cur);
                    cur = next;
                }
            }
            // and the dead strings a string sweep held off `all`
            for h in std::mem::take(&mut self.strt.held) {
                self.free_obj_closing(h);
            }
            // the pooled tables' interiors were freed when they were
            // recycled; only their blocks are left
            for ptr in std::mem::take(&mut self.table_pool) {
                self.free_block(ptr.as_ptr());
            }
            self.free_slab();
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
            hdr: GcHeader {
                aux: self.take_id(),
                ..GcHeader::new(ObjTag::Coro)
            },
            status: crate::runtime::CoroStatus::Suspended,
            body,
            started: false,
            resumer: None,
            resume_at: None,
            error_value: None,
            error_status: crate::runtime::coroutine::ErrorStatus::Run,
            dead: None,
            natives: 0..0,
            stack: LVec::new(mem),
            frames: LVec::new(mem),
            open_upvals: LVec::new(mem),
            tbc: LVec::new(mem),
            top: 0,
            meta_conts: 0,
            stale_frames: 0,
            yield_ci_top: 0,
            stack_size,
            frame_size: if l51 { acct::BASIC_CI_51 } else { u32::MAX },
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

/// A seed of a new heap's own, as [`Heap::new`] picks it.
pub(crate) fn fresh_seed() -> u32 {
    make_seed()
}

/// Hash seed from address entropy (ASLR) and clock, luaL_makeseed style.
pub(super) fn make_seed() -> u32 {
    // the instrumented PUC build takes its seed from `R38_SEED` too
    #[cfg(feature = "gc-trace")]
    if let Some(s) = std::env::var("R38_SEED").ok().and_then(|s| s.parse().ok()) {
        return s;
    }
    let stack_var = 0u8;
    let mut h = &stack_var as *const u8 as u64;
    h ^= (make_seed as *const () as u64) << 16;
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h ^= (d.subsec_nanos() as u64) << 32 ^ d.as_secs();
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h as u32
}
