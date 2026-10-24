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

/// Header prefix on every GC-managed object: intrusive next-link + type tag +
/// mark bits. Always at offset 0 of the containing struct (`#[repr(C)]`).
#[repr(C)]
pub struct GcHeader {
    pub(in crate::runtime) next: *mut GcHeader,
    pub(in crate::runtime) tag: ObjTag,
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
    pub(in crate::runtime) flags: u8,
    /// Per-type byte in what would otherwise be padding: a table keeps the
    /// dialect whose table rules it follows here (`crate::runtime::dialect::Dialect`).
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

pub(in crate::runtime) const WHITE0: u8 = 1;
pub(in crate::runtime) const WHITE1: u8 = 2;
pub(in crate::runtime) const BLACK: u8 = 4;
pub(in crate::runtime) const WHITE_BITS: u8 = WHITE0 | WHITE1;
pub(in crate::runtime) const COLOR_BITS: u8 = WHITE_BITS | BLACK;

/// registered for finalization (`__gc`): the object is tracked in `finalize`.
pub(in crate::runtime) const FIN: u8 = 128;
/// finalization already scheduled/run: never finalize this object again (PUC
/// FINALIZEDBIT). Set when it moves to `tobefnz`.
pub(in crate::runtime) const FINALIZED: u8 = 16;
/// resurrected once because a reference cycle through a coroutine kept the
/// finalizable alive (PUC 5.3 gc.lua :502 "two collections are needed to
/// break cycle"). The next time the object is found unreachable it is moved
/// to `tobefnz` without re-deferring.
pub(in crate::runtime) const DEFERRED: u8 = 32;
/// the object has no children; fixed at creation
pub(in crate::runtime) const LEAF: u8 = 64;

#[inline(always)]
pub(in crate::runtime) fn is_white(flags: u8) -> bool {
    flags & WHITE_BITS != 0
}
#[inline(always)]
pub(in crate::runtime) fn is_black(flags: u8) -> bool {
    flags & BLACK != 0
}

/// True when marking reached the object (PUC `isgray(o) || isblack(o)`).
pub(crate) fn header_is_marked<T: GcObject>(g: Gc<T>) -> bool {
    // SAFETY: `g` is a handle, so its object is allocated (a collect frees only after marking ends), and a `GcObject` starts with its header; only the flag byte is read
    unsafe { !is_white((*g.header()).flags) }
}

mod header;
pub use header::ObjTag;
pub(crate) use header::{
    AUX_OFFSET, EXTERNAL, LUNA_ONLY, READONLY_AUX, RO_CLS, SLAB, SLOW, STANDS, SUB_OFFSET,
    UNCOUNTED, UNLISTED,
};

mod gc_ptr;
pub use gc_ptr::{Gc, GcObject};

/// Incremental GC phase: `Pause` (all objects current-white), `Propagate`
/// (gray queue set, the mutator runs between steps under the barriers,
/// until `gc_finish_atomic`), `Sweep` (`all` swept in place from after
/// `sweep_prev`); `mark_all` runs all of it inline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::runtime) enum GcPhase {
    Pause,
    Propagate,
    Sweep,
}

/// The Propagate phase's state across steps (`Heap.propagate`); the gray
/// queue is `Heap.gray`, which the barriers push to directly.
pub(in crate::runtime) struct PropagateState {
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
    /// the dialect's rules: tables, string hashing, what the collector
    /// keeps and when it finalizes
    rules: crate::runtime::dialect::Rules,
    /// PUC `g->currentwhite`: the white of born / surviving objects; the
    /// other is the dead white the sweep frees. Flipped by `atomic`.
    current_white: u8,
    /// incremental-sweep phase (Pause unless a `step` cycle is mid-sweep)
    phase: GcPhase,
    /// `collectgarbage("stop")`: auto-GC is suspended while true
    gc_stopped: bool,
    /// 5.4 and 5.5: a finalizer is running, which no step may interrupt
    /// (PUC `GCSTPGC`)
    gc_fin_stop: bool,
    /// the heap is being dropped: a string it frees is not taken out of
    /// the string table, which goes with it
    dropping: bool,
    /// a raw table store the interpreter's fast path tried could not grow
    /// the table: the slow path it falls back to counts that as the first
    /// attempt (see `Vm::newindex_miss`)
    store_failed: bool,
    /// gray objects are left off `gray` because it could not grow
    gray_overflow: bool,
    /// natives without upvalues allocated while set go on `fixed` (the
    /// standard library's functions, PUC's light C functions)
    fix_natives: bool,
    track_chunk_roots: bool,
    /// 5.1: the name `__gc`, which a userdata listed for finalization must
    /// have in its metatable when it dies to be finalized (PUC 5.1 lists
    /// every userdata given a metatable and looks then)
    gc_event_51: Option<Gc<crate::runtime::LuaStr>>,
    all: *mut GcHeader,
    /// PUC `fixedgc`: objects never swept, freed with the heap
    fixed: *mut GcHeader,
    /// the head of `all` at the last safe point; objects before it are newer
    young: *mut GcHeader,
    /// `young` as a check where the stack held every live value left it
    /// (`CONCAT`'s): an emergency collection does not keep it for its own sake
    rooted_young: *mut GcHeader,
    /// during `GcPhase::Sweep`: the last object swept and kept, whose
    /// `next` is the next one to sweep (null: the head of `all`). Objects
    /// made since are in front of it and are not swept (PUC `sweepgc`)
    sweep_prev: *mut GcHeader,
    live: usize,
    seed: u32,
    /// Embedding memory cap, soft: over `bytes` between dispatch turns, a full collect, then (still
    /// over) a catchable "memory cap exceeded"; one allocation can pass it.
    mem_cap: Option<usize>,
    /// Persistent gray queue of objects grayed by write barriers, outside `propagate` so barriers
    /// skip the Option; `gc_step_propagate` and `gc_finish_atomic` drain it.
    gray: mark::GrayStack,
    /// objects a back barrier sent to gray (PUC `grayagain`)
    grayagain: mark::GrayStack,
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
    /// `io.stdin` as made, which the host reads lines through; null once freed
    stdin_ud: *mut GcHeader,
    /// 5.1: the main thread, the list's oldest object, after which each new
    /// userdata goes (`luaS_newudata` links it after `mainthread`)
    udata_after: Option<Gc<GcHeader>>,
    /// The main functions of the chunks loaded while `track_chunk_roots`
    /// is set, without keeping them alive: the collector drops each one it
    /// finds dead. Compiled traces taken over from another Vm find the
    /// functions they inlined in these chunks.
    chunk_roots: Vec<Gc<Proto>>,
    /// values and objects the VM refers to without keeping them alive, and
    /// whether the last marking found each dead (`Vm::forget_dead_errors`)
    weak_watch: Vec<(Value, bool)>,
    weak_objs: Vec<(*const GcHeader, bool)>,
    /// the threads PUC's collector visits: since the last marking, the kept
    threads: Vec<Gc<crate::runtime::Coro>>,
    /// PUC's own structures luna has none of (see `runtime::puc`)
    pub(crate) puc: puc::PucModel,
    /// what the builds only tests use keep (`runtime::gc_hooks`)
    pub(crate) hooks: crate::runtime::gc_hooks::HookState,
    strings: StringTable,
    /// Where the heap's memory comes from. Last, so the context outlives
    /// whatever the other fields free through it.
    mem: crate::runtime::mem::MemOwner,
}

impl Heap {
    /// Put a new object on the all-objects list with its born color, or
    /// (while a load builds what PUC makes later) aside until PUC makes it.
    ///
    /// # Safety
    /// `h` is the header of an object the caller just allocated (a fresh box
    /// or a recycled pool table) and has not linked anywhere yet, so this is
    /// the only pointer to it.
    #[inline]
    unsafe fn link(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if self.loads_set_aside() && self.set_aside(h) {
                return;
            }
            self.link_now(h);
        }
    }
}

use crate::runtime::puc;
pub(crate) use crate::runtime::puc::records::{Ghost, GhostRef, parse, strcache};
use crate::runtime::puc::records::{env51, open_upvals, strt};
pub(crate) use crate::runtime::puc::{Deferred, PucKind, PucObj, Thread};
mod alloc;
mod barrier;
#[cfg(any(debug_assertions, feature = "gc-verify", feature = "gc-stress"))]
mod checks;
mod close;
mod collect;
pub(crate) use collect::{Propagated, Running};
mod finalize;
mod for_vm;
mod intern;
mod list;
mod new;
mod slab;
use self::intern::oom_abort_heap;
pub(crate) use self::new::fresh_seed;
use self::new::make_seed;
pub(crate) use env51::{ENV_IO, ENV_IO_POPEN, ENV_IO_STD, ENV_PACKAGE, ENV_PROTO};
mod free;
mod mark;
pub(crate) use open_upvals::OpenLend;
mod pace;
mod sweep;
#[cfg(feature = "gc-trace")]
mod trace;
pub(crate) use self::mark::{Marker, native_puc_upvals};
use self::mark::{blacken, drain_marker, survived_mark, weak_key_alive};

#[cfg(feature = "gc-verify")]
pub(crate) mod verify;

#[cfg(test)]
mod tests;
