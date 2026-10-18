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
use crate::runtime::table::{self, Table};
use crate::runtime::userdata::{Userdata, UserdataPayload};
use crate::runtime::value::Value;

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
    /// Class bits, in padding: `UNLISTED`, `STANDS`, `RO_CLS`, `SLAB`, ...
    pub(crate) cls: u8,
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

#[inline(always)]
fn is_white(flags: u8) -> bool {
    flags & WHITE_BITS != 0
}
#[inline(always)]
fn is_black(flags: u8) -> bool {
    flags & BLACK != 0
}

/// True when marking reached the object (PUC `isgray(o) || isblack(o)`).
pub(crate) fn header_is_marked<T: GcObject>(g: Gc<T>) -> bool {
    // SAFETY: `g` is a handle, so its object is allocated (a collect frees only after marking ends), and a `GcObject` starts with its header; only the flag byte is read
    unsafe { !is_white((*g.header()).flags) }
}

#[path = "heap_header.rs"]
mod header;
pub use header::ObjTag;
pub(crate) use header::{
    AUX_OFFSET, EXTERNAL, LUNA_ONLY, READONLY_AUX, RO_CLS, SLAB, SLOW, STANDS, SUB_OFFSET,
    UNCOUNTED, UNLISTED,
};

#[path = "gc_ptr.rs"]
mod gc_ptr;
pub use gc_ptr::{Gc, GcObject};

/// Incremental GC phase: `Pause` (no cycle, all objects current-white),
/// `Propagate` (the gray queue and propagate state are set; the mutator runs
/// between steps, new objects current-white, barriers marking children or
/// sending parents back to gray; `gc_finish_atomic` ends it), `Sweep` (`all`
/// swept in place from after `sweep_prev`). `mark_all` (the stop-the-world
/// path) runs all of it inline, never crossing a step boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GcPhase {
    Pause,
    Propagate,
    Sweep,
}

/// Cross-step traversal state of the Propagate phase (`Heap.propagate`, set
/// from `gc_start_propagate` to `gc_finish_atomic`); the gray queue is in
/// `Heap.gray` so write barriers push without going through the Option.
struct PropagateState {
    quiet: Vec<*mut GcHeader>,
    weak: Vec<*mut Table>,
    ephemeron: Vec<*mut Table>,
    cached_protos: Vec<*mut Proto>,
    no_ephemeron: bool,
}

/// luna's incremental mark-sweep GC heap (one per Vm): owns every [`Gc<T>`]
/// allocation, the string-intern table and the auto-GC pacing state.
// the fields every allocation and store reads first, by hand: the
// compiler's own order moved them apart when unrelated fields changed
#[repr(C)]
pub struct Heap {
    /// PUC's `totalbytes`: every object and block at the size PUC gives it
    /// (`sizes`), not what luna's own objects take (see `heap_acct.rs`)
    pub(crate) bytes: usize,
    /// where the next collection step falls due (`pace`), or `usize::MAX`
    /// while there is none: the one comparison a safe point makes
    gc_limit: usize,
    /// PUC `g->currentwhite`: which white bit (WHITE0 or WHITE1) means
    /// "born / surviving this cycle". The other white is the dead-white that
    /// sweep collects. Flipped at the end of each mark cycle (`atomic`).
    current_white: u8,
    /// incremental-sweep phase (Pause unless a `step` cycle is mid-sweep)
    phase: GcPhase,
    /// `collectgarbage("stop")`: auto-GC is suspended while true
    gc_stopped: bool,
    /// 5.4 and 5.5: a finalizer is running, which no step may interrupt
    /// (PUC `GCSTPGC`)
    pub(crate) gc_fin_stop: bool,
    /// the heap is being dropped: a string it frees is not taken out of
    /// the string table, which goes with it
    dropping: bool,
    /// a raw table store the interpreter's fast path tried could not grow
    /// the table: the slow path it falls back to counts that as the first
    /// attempt (see `Vm::newindex_miss`)
    pub(crate) store_failed: bool,
    /// gray objects are left off `gray` because it could not grow
    gray_overflow: bool,
    /// natives without upvalues allocated while set go on `fixed` (the
    /// standard library's functions, PUC's light C functions)
    pub(crate) fix_natives: bool,
    pub(crate) track_chunk_roots: bool,
    /// PUC 5.1 has no ephemeron pass: a `__mode='k'` table marks its values
    /// strongly, so `a[t]=t` survives with nothing else referencing `t`
    /// (5.1's gc.lua "weak tables" asserts 3*lim survivors, 5.4's 2*lim).
    pub(crate) no_ephemeron: bool,
    /// 5.1/5.2: a new table key -0 stays -0 (see `Table::set`)
    pub(crate) signed_zero_keys: bool,
    /// hash strings with PUC 5.1's function (`string::lua_hash_51`)
    pub(crate) hash51: bool,
    /// the dialect whose table rules (array sizing, hashing of number
    /// keys, the length border) a new table follows
    pub(crate) table_dialect: crate::runtime::table::Dialect,
    /// PUC 5.3 finalizes a table in a cycle through an unreachable coroutine
    /// one collection after it finds it unreachable (gc.lua :502); 5.4 and
    /// 5.5 in the same one (gc.lua :544). Set by the VM at construction.
    pub(crate) defer_thread_cycle_finalize: bool,
    /// 5.1: the name `__gc`, which a userdata listed for finalization must
    /// have in its metatable when it dies to be finalized (PUC 5.1 lists
    /// every userdata given a metatable and looks then)
    pub(crate) gc_event_51: Option<Gc<crate::runtime::LuaStr>>,
    /// An object finalized once can be registered again (all but 5.2,
    /// whose `luaC_checkfinalizer` refuses a finalized object).
    pub(crate) refinalize: bool,
    all: *mut GcHeader,
    /// PUC `fixedgc`: objects never swept, freed with the heap
    fixed: *mut GcHeader,
    /// the head of `all` at the last point where every live object was
    /// rooted; objects before it on `all` are newer (see `mark_young`)
    young: *mut GcHeader,
    /// during `GcPhase::Sweep`: the last object swept and kept, whose
    /// `next` is the next one to sweep (null: the head of `all`). Objects
    /// made since are in front of it and are not swept (PUC `sweepgc`)
    sweep_prev: *mut GcHeader,
    live: usize,
    /// PUC's collector state and pace
    pub(crate) pace: crate::runtime::gc_pace::Pace,
    seed: u32,
    /// Embedding memory cap, soft: between dispatch turns the run loop
    /// checks `bytes` and over it runs a full collect, then (still over)
    /// raises a catchable "memory cap exceeded"; one allocation can pass it.
    pub(crate) mem_cap: Option<usize>,
    /// Persistent gray queue: holds objects grayed by write barriers between
    /// the time the marker first reached them and the next propagate step.
    /// Lives outside `propagate` so barriers can push without going through
    /// the Option; `gc_step_propagate` and `gc_finish_atomic` drain it.
    gray: mark::GrayStack,
    /// objects a back barrier sent to gray (PUC `grayagain`)
    grayagain: mark::GrayStack,
    /// 5.1, 5.2: the sweep of PUC's string table under way
    strt: strt::StrtSweep,
    /// objects registered for finalization (a live `__gc` metamethod was set);
    /// parallel-tracked — ownership stays on `all` (PUC `finobj`).
    finalize: crate::runtime::mem::LVec<*mut GcHeader>,
    /// dead finalizables resurrected this cycle, awaiting their `__gc` call by
    /// the VM (PUC `tobefnz`); the entries before `tobefnz_head` were handed
    /// out already (`take_one_tobefnz`).
    tobefnz: crate::runtime::mem::LVec<*mut GcHeader>,
    tobefnz_head: usize,
    /// Incremental traversal state. `Some` between `gc_start_propagate` and
    /// `gc_finish_atomic` (and inline within `mark_all`); `None` otherwise.
    propagate: Option<PropagateState>,
    /// Freed tables (at most 4096) that new tables reuse, listed outside
    /// the allocation context: a sweep allocates nothing, as PUC's.
    table_pool: Vec<std::ptr::NonNull<crate::runtime::table::Table>>,
    /// the blocks the fixed natives are made in
    slab: slab::NativeSlab,
    /// 5.1's environments luna has no objects for
    env51_box: Option<Box<env51::Env51>>,
    /// The main functions of the chunks loaded while `track_chunk_roots`
    /// is set, without keeping them alive: the collector drops each one it
    /// finds dead. Compiled traces taken over from another Vm find the
    /// functions they inlined in these chunks.
    pub(crate) chunk_roots: Vec<Gc<Proto>>,
    /// values the VM refers to without keeping them alive, and whether the
    /// last marking found each dead (see `Vm::forget_dead_errors`)
    pub(crate) weak_watch: Vec<(Value, bool)>,
    /// the threads PUC's collector visits: since the last marking, the kept
    pub(crate) threads: Vec<Gc<crate::runtime::Coro>>,
    /// the sizes of PUC's objects in this dialect, which `bytes` counts
    pub(crate) sizes: crate::runtime::puc_size::PucSizes,
    /// PUC's own structures luna has none of (see `heap_acct.rs`)
    pub(crate) puc: acct::PucState,
    #[cfg(feature = "gc-trace")]
    pub(crate) trace: crate::runtime::gc_trace::Trace,
    /// `gc-verify` — headers freed since the last collect
    /// began. O(1) read-time dangling probes (`Vm::op_index`) test
    /// membership here; cleared when the next mark starts. Only exact
    /// under ASAN-style quarantining allocators (no immediate reuse).
    #[cfg(feature = "gc-verify")]
    pub(crate) recently_freed: std::collections::HashSet<usize>,
    strings: StringTable,
    /// Where the heap's memory comes from. Last, so the context outlives
    /// whatever the other fields free through it.
    mem: crate::runtime::mem::MemOwner,
}

impl Heap {
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
            // born the current white in every phase (PUC `luaC_white(g)`):
            // one made while marking is marked by the atomic step if it is
            // reachable then (the roots are marked again there, and a black
            // object that took it was sent back to gray by its barrier)
            (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.current_white);
        }
        self.all = h;
        self.live += 1;
        #[cfg(feature = "gc-verify")]
        self.forget_freed(h);
    }
}

#[path = "heap_acct.rs"]
mod acct;
pub(crate) use acct::{Deferred, Ghost, GhostRef, PucKind, is_uncounted};
#[path = "heap_alloc.rs"]
mod alloc;
#[path = "heap_barrier.rs"]
mod barrier;
#[path = "heap_collect.rs"]
mod collect;
pub(crate) use collect::{Propagated, Running};
#[path = "heap_env51.rs"]
mod env51;
#[path = "heap_finalize.rs"]
mod finalize;
#[path = "heap_intern.rs"]
mod intern;
#[path = "heap_new.rs"]
mod new;
#[path = "heap_slab.rs"]
mod slab;
use self::intern::oom_abort_heap;
pub(crate) use self::new::fresh_seed;
use self::new::make_seed;
pub(crate) use env51::{ENV_IO, ENV_IO_POPEN, ENV_IO_STD, ENV_PACKAGE, ENV_PROTO};
#[path = "heap_free.rs"]
mod free;
#[path = "heap_mark.rs"]
mod mark;
#[path = "heap_pace.rs"]
mod pace;
#[path = "heap_strt.rs"]
mod strt;
#[path = "heap_sweep.rs"]
mod sweep;
#[cfg(feature = "gc-trace")]
#[path = "heap_trace.rs"]
mod trace;
pub(crate) use self::mark::{Marker, native_puc_upvals};
use self::mark::{blacken, drain_marker, survived_mark, weak_key_alive};

#[cfg(feature = "gc-verify")]
#[path = "heap_verify.rs"]
mod verify;

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
