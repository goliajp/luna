//! GC heap v1: precise stop-the-world mark & sweep over an intrusive
//! all-objects list (PUC `allgc` shape). All unsafe object plumbing is
//! confined to this module and `string`/`table` internals.
//!
//! `Gc<T>` safety contract: the runtime is single-threaded; a `Gc` pointer is
//! valid until a `collect()` call that does not reach it from the given
//! roots. Callers must root every value they keep across a collect.

use std::ptr;

use crate::runtime::function::{LuaClosure, NativeClosure, Proto, UpvalState, Upvalue};
use crate::runtime::string::{self, LuaStr, StringTable};
use crate::runtime::table::Table;
use crate::runtime::userdata::{Userdata, UserdataPayload};
use crate::runtime::value::Value;

/// Discriminator the GC stores in every [`GcHeader`] so a raw header pointer
/// can be cast back to the right object kind during tracing and sweeping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ObjTag {
    /// [`crate::runtime::string::LuaStr`].
    Str,
    /// [`crate::runtime::table::Table`].
    Table,
    /// [`crate::runtime::function::Proto`].
    Proto,
    /// [`crate::runtime::function::LuaClosure`].
    Closure,
    /// [`crate::runtime::function::Upvalue`].
    Upvalue,
    /// [`crate::runtime::function::NativeClosure`].
    Native,
    /// [`crate::runtime::coroutine::Coro`].
    Coro,
    /// [`crate::runtime::userdata::Userdata`].
    Userdata,
}

/// Header prefix on every GC-managed object: intrusive next-link + type tag +
/// mark bits. Always at offset 0 of the containing struct (`#[repr(C)]`).
#[repr(C)]
pub struct GcHeader {
    next: *mut GcHeader,
    tag: ObjTag,
    /// tricolor + finalizer state. PUC `gch.marked` layout (lgc.h):
    ///   bit 0 WHITE0 — current-white-A
    ///   bit 1 WHITE1 — current-white-B (the unused white in any given cycle
    ///                  is the "other-white" / dead-white at sweep time)
    ///   bit 2 BLACK  — propagated; outgoing refs already traced
    ///   bit 3 SLOW   — set exactly when BLACK is set or the object is a
    ///                  read-only table (`READONLY_AUX`): an in-place table
    ///                  store tests this one bit (`plain_store`) and leaves
    ///                  both cases to the slow path. Every colour change goes
    ///                  through `with_slow`, which keeps it in step
    ///   bit 4 FINALIZED — already enqueued or finalized once this lifetime
    ///   bit 5 DEFERRED  — 5.3 cycle-finalize deferral marker (gc.lua :502)
    ///   bit 6 LEAF   — nothing to trace (a string, a native without upvalues)
    ///   bit 7 FIN    — registered for `__gc` (tracked in `finalize`)
    /// Gray = no white bits, no BLACK; that is the in-stack state between the
    /// time a Marker visits an object and the time it traces it.
    flags: u8,
    /// Per-type byte in what would otherwise be padding: a table keeps the
    /// dialect whose table rules it follows here (`table::Dialect`).
    pub(crate) sub: u8,
    /// Per-type word in what would otherwise be padding: a table keeps its
    /// absent-metamethod bits here. Zero for a new object.
    pub(crate) aux: u32,
}

// strings are the most numerous objects the sweep walks
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<LuaStr>() == 32);

const WHITE0: u8 = 1;
const WHITE1: u8 = 2;
const BLACK: u8 = 4;
const WHITE_BITS: u8 = WHITE0 | WHITE1;
const COLOR_BITS: u8 = WHITE_BITS | BLACK;

/// registered for finalization (`__gc`): the object is tracked in `finalize`.
const FIN: u8 = 128;
/// finalization already scheduled/run: never finalize this object again (PUC
/// FINALIZEDBIT). Set when it moves to `tobefnz`.
const FINALIZED: u8 = 16;
/// resurrected once because a reference cycle through a coroutine kept the
/// finalizable alive (PUC 5.3 gc.lua :502 "two collections are needed to
/// break cycle"). The next time the object is found unreachable it is moved
/// to `tobefnz` without re-deferring.
const DEFERRED: u8 = 32;
/// the object has no children; fixed at creation
const LEAF: u8 = 64;
/// BLACK or a read-only table; see the flag layout on `GcHeader`
const SLOW: u8 = 8;
/// The `aux` bit of a table that marks it read-only (`Table::is_readonly`).
/// A table's other `aux` bits are its absent-metamethod bits, one per event
/// below this one; the JIT's inline stores test this bit in the byte of
/// `aux` that holds it.
pub(crate) const READONLY_AUX: u32 = 1 << 31;

/// Byte offset of the `aux` word within `GcHeader`, for the JIT's
/// read-only test.
pub(crate) const AUX_OFFSET: usize = std::mem::offset_of!(GcHeader, aux);
pub(crate) const SUB_OFFSET: usize = std::mem::offset_of!(GcHeader, sub);

#[inline(always)]
fn is_white(flags: u8) -> bool {
    flags & WHITE_BITS != 0
}
#[inline(always)]
fn is_black(flags: u8) -> bool {
    flags & BLACK != 0
}

/// True when an object header has been reached by marking (gray or black).
/// `pub(crate)` so other runtime modules (e.g. `Table::refs_contain_unmarked_coro`)
/// can probe reachability without owning the bit constants. Equivalent to
/// `isgray(o) || isblack(o)` in PUC.
pub(crate) fn header_is_marked<T: GcObject>(g: Gc<T>) -> bool {
    // SAFETY: `g` is a handle, so its object is allocated (a collect frees only after marking ends), and a `GcObject` starts with its header; only the flag byte is read
    unsafe { !is_white((*g.header()).flags) }
}

#[path = "heap_header.rs"]
mod header;

#[path = "gc_ptr.rs"]
mod gc_ptr;
pub use gc_ptr::{Gc, GcObject};

/// Incremental GC phase.
///   * `Pause`     — no cycle in progress; all objects current-white.
///   * `Propagate` — gray queue + propagate-state populated; mutator runs
///                   alongside `gc_step_propagate(budget)` calls. Born objects
///                   stamp the current-white; barriers re-gray modified
///                   parents. Transitions to `Sweep` via `gc_finish_atomic`.
///   * `Sweep`     — `sweep_cur` is the detached old heap being budget-swept.
///
/// `mark_all` (the STW path used by `collect_ex`) sequences start_propagate +
/// drain_all + finish_atomic inline, never crossing a step boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GcPhase {
    Pause,
    Propagate,
    Sweep,
}

/// Cross-step traversal state for the incremental Propagate phase. Owned by
/// `Heap.propagate` (`Some` between `gc_start_propagate` and `gc_finish_atomic`,
/// `None` otherwise). The gray queue itself lives in `Heap.gray` so write
/// barriers can push directly without going through the Option.
struct PropagateState {
    weak: Vec<*mut Table>,
    ephemeron: Vec<*mut Table>,
    cached_protos: Vec<*mut Proto>,
    no_ephemeron: bool,
}

/// luna's incremental mark-sweep GC heap. Owns every [`Gc<T>`] allocation
/// (one per Vm); produces handles via the `new_*` constructors and traces
/// reachability through registered roots. Holds the string-intern table and
/// the auto-GC pacing state.
pub struct Heap {
    all: *mut GcHeader,
    /// PUC `fixedgc`: objects never swept, freed with the heap
    fixed: *mut GcHeader,
    /// natives without upvalues allocated while set go on `fixed` (the
    /// standard library's functions, PUC's light C functions)
    pub(crate) fix_natives: bool,
    strings: StringTable,
    seed: u32,
    live: usize,
    /// approximate allocated bytes — shells only. Each `link()` adds
    /// `size_of::<T>` for its tag; each `free_obj` subtracts the same.
    /// Internal Vec/Box growth (table array/hash parts, proto code,
    /// closure upvals slice) is NOT auto-tracked, so this is a lower
    /// bound on real memory. PUC's `g->GCtotalbytes` is exact because
    /// `lmem.c` routes every malloc/free through one helper; luna pays
    /// for that uniformity in exchange for a smaller, drift-free count.
    bytes: usize,
    /// byte threshold at which the VM should run a collection (auto-GC pacing)
    next_gc: usize,
    /// `next_gc`, or `usize::MAX` while auto-GC is stopped: the one
    /// comparison a safe point makes
    gc_limit: usize,
    /// PUC `g->currentwhite`: which white bit (WHITE0 or WHITE1) means
    /// "born / surviving this cycle". The other white is the dead-white that
    /// sweep collects. Flipped at the end of each mark cycle (`atomic`).
    current_white: u8,
    /// Persistent gray queue: holds objects grayed by write barriers between
    /// the time the marker first reached them and the next propagate step.
    /// Lives outside `propagate` so barriers can push without going through
    /// the Option; `gc_step_propagate` and `gc_finish_atomic` drain it.
    gray: Vec<*mut GcHeader>,
    /// Incremental traversal state. `Some` between `gc_start_propagate` and
    /// `gc_finish_atomic` (and inline within `mark_all`); `None` otherwise.
    propagate: Option<PropagateState>,
    /// incremental-sweep phase (Pause unless a `step` cycle is mid-sweep)
    phase: GcPhase,
    /// the remaining detached object list being swept during `GcPhase::Sweep`;
    /// survivors are spliced back onto `all`, garbage is freed
    sweep_cur: *mut GcHeader,
    /// `collectgarbage("stop")`: auto-GC is suspended while true
    gc_stopped: bool,
    /// objects registered for finalization (a live `__gc` metamethod was set);
    /// parallel-tracked — ownership stays on `all` (PUC `finobj`).
    finalize: Vec<*mut GcHeader>,
    /// dead finalizables resurrected this cycle, awaiting their `__gc` call by
    /// the VM (PUC `tobefnz`). Drained via `take_tobefnz`.
    tobefnz: Vec<*mut GcHeader>,
    /// PUC 5.1 has no ephemeron pass: a `__mode='k'` table marks its values
    /// strongly during traversal, so entries like `a[t]=t` (key and value the
    /// same fresh object) survive even with nothing else referencing `t`.
    /// 5.2+ replaced that with ephemeron convergence. gc.lua's "weak tables"
    /// section in 5.1 asserts 3*lim survivors, 5.4 only 2*lim — the loop2
    /// pair was retired from the newer test as a result.
    pub(crate) no_ephemeron: bool,
    /// 5.1/5.2: a new table key -0 stays -0 (see `Table::set`)
    pub(crate) signed_zero_keys: bool,
    /// the dialect whose table rules (array sizing, hashing of number
    /// keys, the length border) a new table follows
    pub(crate) table_dialect: crate::runtime::table::Dialect,
    /// hash strings with PUC 5.1's function (`string::lua_hash_51`)
    pub(crate) hash51: bool,
    /// PUC 5.3 finalizes a table caught in a cycle through an unreachable
    /// coroutine one GC round later than the unreachability is detected
    /// ("two collections are needed to break cycle", gc.lua :502). 5.4 and 5.5
    /// rewrote the GC and finalize the same cycle in a single pass (their
    /// gc.lua :544 asserts collected after one `collectgarbage()`). 5.1/5.2
    /// don't exercise this path. Set by the VM at construction.
    pub(crate) defer_thread_cycle_finalize: bool,
    /// The main functions of the chunks loaded while `track_chunk_roots`
    /// is set, without keeping them alive: the collector drops each one it
    /// finds dead. Compiled traces taken over from another Vm find the
    /// functions they inlined in these chunks.
    pub(crate) chunk_roots: Vec<Gc<Proto>>,
    pub(crate) track_chunk_roots: bool,
    /// Pool of freed Table allocations.
    /// btrees-style workloads create + free ~32k tables per iter;
    /// jemalloc's malloc/free roundtrip costs ~30ns per table = ~960µs
    /// total per iter. Pool recycle: free_obj pushes the raw Table
    /// pointer here instead of dropping; new_table pops + resets fields.
    /// Cap at 4096 entries to avoid unbounded growth (worst-case: 4096
    /// × sizeof(Table) ≈ 460 KB resident memory in idle pool).
    table_pool: Vec<std::ptr::NonNull<crate::runtime::table::Table>>,
    /// `gc-verify` — headers freed since the last collect
    /// began. O(1) read-time dangling probes (`Vm::op_index`) test
    /// membership here; cleared when the next mark starts. Only exact
    /// under ASAN-style quarantining allocators (no immediate reuse).
    #[cfg(feature = "gc-verify")]
    pub(crate) recently_freed: std::collections::HashSet<usize>,
    /// Embedding memory cap. When `Some(n)`, the VM's run loop watches
    /// `bytes` between dispatch turns and, on overshoot, runs a full collect
    /// and (still overshooting) raises a catchable "memory cap exceeded"
    /// Lua error. A soft cap, not a hard alloc-time refusal: a single
    /// allocation can briefly push `bytes` past `n`, but the embedder gets
    /// control back at the next safe point — host policy.
    pub(crate) mem_cap: Option<usize>,
    /// Where the heap's memory comes from. Last, so the context outlives
    /// whatever the other fields free through it.
    mem: crate::runtime::mem::MemOwner,
}

/// Initial auto-GC threshold and floor (PUC GCSTEPSIZE-ish pacing).
const GC_MIN_THRESHOLD: usize = 1 << 20;

impl Heap {
    /// Build a fresh empty heap with default GC pacing and no memory cap.
    pub fn new() -> Heap {
        Heap::with_seed(make_seed())
    }

    /// Whether no string has been interned yet.
    pub(crate) fn strings_is_empty(&self) -> bool {
        self.strings.is_empty()
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
        Heap {
            all: ptr::null_mut(),
            fixed: ptr::null_mut(),
            fix_natives: false,
            strings: StringTable::new(),
            seed,
            live: 0,
            bytes: 0,
            next_gc: GC_MIN_THRESHOLD,
            gc_limit: GC_MIN_THRESHOLD,
            current_white: WHITE0,
            gray: Vec::new(),
            propagate: None,
            phase: GcPhase::Pause,
            sweep_cur: ptr::null_mut(),
            gc_stopped: false,
            finalize: Vec::new(),
            tobefnz: Vec::new(),
            no_ephemeron: false,
            signed_zero_keys: false,
            table_dialect: crate::runtime::table::Dialect::L55,
            hash51: false,
            defer_thread_cycle_finalize: false,
            chunk_roots: Vec::new(),
            track_chunk_roots: false,
            mem_cap: None,
            table_pool: Vec::new(),
            #[cfg(feature = "gc-verify")]
            recently_freed: std::collections::HashSet::new(),
            mem,
        }
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

    /// Put a new object on the all-objects list with its born color.
    ///
    /// # Safety
    /// `h` is the header of an object the caller just allocated (a fresh box
    /// or a recycled pool table) and has not linked anywhere yet, so this is
    /// the only pointer to it.
    #[inline]
    unsafe fn link(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            (*h).next = self.all;
            // Born color depends on phase:
            //   * Pause / Sweep — born current-white (PUC `luaC_white(g)`);
            //     reachable from roots gets marked next cycle.
            //   * Propagate     — born BLACK (PUC `LUAGCRYOUNG` / sasimpl
            //     of new-during-cycle). Born-current-white during Propagate
            //     would lose the WHITE bits at the upcoming atomic flip and
            //     be swept this same cycle even when reachable from a
            //     barrier-grayed root. Born BLACK skips the trace and
            //     transitions to current-white at sweep, matching the
            //     reachable-survivor flow.
            let born = if self.phase == GcPhase::Propagate {
                BLACK
            } else {
                self.current_white
            };
            (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | born);
        }
        self.all = h;
        self.live += 1;
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
            for ptr in std::mem::take(&mut self.table_pool) {
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

/// Hash seed from address entropy (ASLR) and clock, luaL_makeseed style.
fn make_seed() -> u32 {
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

#[path = "heap_alloc.rs"]
mod alloc;
#[path = "heap_barrier.rs"]
mod barrier;
#[path = "heap_collect.rs"]
mod collect;
#[path = "heap_finalize.rs"]
mod finalize;
#[path = "heap_mark.rs"]
mod mark;
#[path = "heap_pace.rs"]
mod pace;
#[path = "heap_sweep.rs"]
mod sweep;
pub(crate) use self::mark::Marker;
use self::mark::{drain_marker, weak_key_alive};

#[cfg(feature = "gc-verify")]
#[path = "heap_verify.rs"]
mod verify;

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
