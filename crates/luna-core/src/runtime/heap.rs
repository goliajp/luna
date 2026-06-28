//! GC heap v1: precise stop-the-world mark & sweep over an intrusive
//! all-objects list (PUC `allgc` shape). All unsafe object plumbing is
//! confined to this module and `string`/`table` internals.
//!
//! `Gc<T>` safety contract: the runtime is single-threaded; a `Gc` pointer is
//! valid until a `collect()` call that does not reach it from the given
//! roots. Callers must root every value they keep across a collect.

use std::fmt;
use std::ops::Deref;
use std::ptr::{self, NonNull};

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
    ///   bit 3 FIN    — registered for `__gc` (tracked in `finalize`)
    ///   bit 4 FINALIZED — already enqueued or finalized once this lifetime
    ///   bit 5 DEFERRED  — 5.3 cycle-finalize deferral marker (gc.lua :502)
    /// Gray = no white bits, no BLACK; that is the in-stack state between the
    /// time a Marker visits an object and the time it traces it.
    flags: u8,
}

const WHITE0: u8 = 1;
const WHITE1: u8 = 2;
const BLACK: u8 = 4;
const WHITE_BITS: u8 = WHITE0 | WHITE1;
const COLOR_BITS: u8 = WHITE_BITS | BLACK;

/// registered for finalization (`__gc`): the object is tracked in `finalize`.
const FIN: u8 = 8;
/// finalization already scheduled/run: never finalize this object again (PUC
/// FINALIZEDBIT). Set when it moves to `tobefnz`.
const FINALIZED: u8 = 16;
/// resurrected once because a reference cycle through a coroutine kept the
/// finalizable alive (PUC 5.3 gc.lua :502 "two collections are needed to
/// break cycle"). The next time the object is found unreachable it is moved
/// to `tobefnz` without re-deferring.
const DEFERRED: u8 = 32;

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
pub(crate) fn header_is_marked(h: *mut GcHeader) -> bool {
    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
    unsafe { !is_white((*h).flags) }
}

impl GcHeader {
    pub(crate) fn new(tag: ObjTag) -> GcHeader {
        GcHeader {
            next: ptr::null_mut(),
            tag,
            flags: 0,
        }
    }
}

/// `Copy` handle to a heap-allocated GC-managed object. Layout is a single
/// `NonNull<T>`; the GC walks reachability via root scanning and intrusive
/// linkage on [`GcHeader`], not via reference counts.
pub struct Gc<T> {
    ptr: NonNull<T>,
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Gc<T> {}

impl<T> Gc<T> {
    #[doc(hidden)]
    pub fn from_ptr(p: *mut T) -> Gc<T> {
        Gc {
            ptr: NonNull::new(p).expect("gc pointer must be non-null"),
        }
    }

    /// Raw pointer to the referent. Always non-null; valid for the lifetime
    /// of the [`Heap`] that allocated it as long as the object is reachable.
    pub fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Pointer-identity equality (PUC `rawequal` for reference types).
    pub fn ptr_eq(self, other: Gc<T>) -> bool {
        self.ptr == other.ptr
    }

    /// SAFETY: caller must ensure no other live reference to the object and
    /// no collect() while the borrow is held (single-threaded runtime).
    ///
    /// `#[doc(hidden)]` (Track A4 — pub-surface 0 unsafe): embedders should
    /// not see this in rustdoc. The safe path for mutating freshly-allocated
    /// tables is the `TableBuilder` / `vm.table_of(...)` API (Track B3).
    /// Cross-crate access from `luna` (e.g. `jit_backend`, `capi`) keeps
    /// working — `#[doc(hidden)] pub` doesn't demote visibility, just docs.
    #[doc(hidden)]
    pub unsafe fn as_mut<'a>(self) -> &'a mut T {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { &mut *self.ptr.as_ptr() }
    }
}

impl<T> Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gc({:p})", self.ptr.as_ptr())
    }
}

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
    /// PUC 5.3 finalizes a table caught in a cycle through an unreachable
    /// coroutine one GC round later than the unreachability is detected
    /// ("two collections are needed to break cycle", gc.lua :502). 5.4 and 5.5
    /// rewrote the GC and finalize the same cycle in a single pass (their
    /// gc.lua :544 asserts collected after one `collectgarbage()`). 5.1/5.2
    /// don't exercise this path. Set by the VM at construction.
    pub(crate) defer_thread_cycle_finalize: bool,
    /// P17-D v2 layer-15 attack: pool of freed Table allocations.
    /// btrees-style workloads create + free ~32k tables per iter;
    /// jemalloc's malloc/free roundtrip costs ~30ns per table = ~960µs
    /// total per iter. Pool recycle: free_obj pushes the raw Table
    /// pointer here instead of dropping; new_table pops + resets fields.
    /// Cap at 4096 entries to avoid unbounded growth (worst-case: 4096
    /// × sizeof(Table) ≈ 460 KB resident memory in idle pool).
    table_pool: Vec<std::ptr::NonNull<crate::runtime::table::Table>>,
    /// P09 embedding memory cap. When `Some(n)`, the VM's run loop watches
    /// `bytes` between dispatch turns and, on overshoot, runs a full collect
    /// and (still overshooting) raises a catchable "memory cap exceeded"
    /// Lua error. A soft cap, not a hard alloc-time refusal: a single
    /// allocation can briefly push `bytes` past `n`, but the embedder gets
    /// control back at the next safe point — host policy.
    pub(crate) mem_cap: Option<usize>,
}

/// Initial auto-GC threshold and floor (PUC GCSTEPSIZE-ish pacing).
const GC_MIN_THRESHOLD: usize = 1 << 20;

impl Heap {
    /// Build a fresh empty heap with default GC pacing and no memory cap.
    pub fn new() -> Heap {
        Heap {
            all: ptr::null_mut(),
            strings: StringTable::new(),
            seed: make_seed(),
            live: 0,
            bytes: 0,
            next_gc: GC_MIN_THRESHOLD,
            current_white: WHITE0,
            gray: Vec::new(),
            propagate: None,
            phase: GcPhase::Pause,
            sweep_cur: ptr::null_mut(),
            gc_stopped: false,
            finalize: Vec::new(),
            tobefnz: Vec::new(),
            no_ephemeron: false,
            defer_thread_cycle_finalize: false,
            mem_cap: None,
            table_pool: Vec::new(),
        }
    }

    fn link(&mut self, h: *mut GcHeader) {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
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
            (*h).flags = ((*h).flags & !COLOR_BITS) | born;
        }
        self.all = h;
        self.live += 1;
    }

    /// Take ownership of a boxed object and put it under GC management.
    /// SAFETY-by-convention: `T` must be `repr(C)` with a `GcHeader` first
    /// field whose tag matches `T` (enforced by the typed constructors).
    pub(crate) fn adopt<T>(&mut self, obj: Box<T>) -> Gc<T> {
        let p = Box::into_raw(obj);
        self.link(p as *mut GcHeader);
        self.bytes += std::mem::size_of::<T>();
        Gc::from_ptr(p)
    }

    /// Allocate and adopt a fresh empty [`Table`].
    pub fn new_table(&mut self) -> Gc<Table> {
        // P17-D v2 layer-15 attack — table_pool fast path. When btrees-
        // style alloc bursts have left freed Tables in the pool, pop a
        // recycled one and reset its fields instead of mallocing fresh.
        // Saves ~30ns per alloc (malloc roundtrip elided).
        let p = if let Some(ptr) = self.table_pool.pop() {
            let t = ptr.as_ptr();
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe {
                // Reset to fresh-Table state. Box-owned slab/nodes/
                // metatable were already cleared in `free_obj` before
                // pool push, so we only reset stack-resident fields here.
                (*t).hdr = GcHeader::new(ObjTag::Table);
                (*t).array_ptr = std::ptr::null_mut();
                (*t).asize = 0;
                (*t).inline_storage = [0; crate::runtime::table::INLINE_U64S];
                (*t).lastfree = 0;
                (*t).flags = 0;
            }
            t
        } else {
            Box::into_raw(Box::new(Table::new(GcHeader::new(ObjTag::Table))))
        };
        // Link + bytes accounting (same as adopt path).
        self.link(p as *mut GcHeader);
        self.bytes += std::mem::size_of::<Table>();
        let g = Gc::from_ptr(p);
        // P11-S5d.I — the Table is now at its final heap address; wire
        // `array_ptr` to point at the inline storage that lives inside
        // the boxed Table.
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { g.as_mut() }.init_array_ptr();
        g
    }

    /// P11-S5c.B — adopt an empty table and pre-allocate `asize`
    /// NIL slots in the array part. Equivalent to `new_table()`
    /// followed by `set_int(N, Nil)` worth of `rehash`es, except
    /// the array reaches its final size in one allocation rather
    /// than O(log N) doubling rounds.
    ///
    /// `asize == 0` is identical to `new_table()`. Larger sizes
    /// are clamped at the array part's hard ceiling
    /// `MAX_ASIZE = 2^27`; requests beyond that fall back to the
    /// empty table, which the interpreter would have grown
    /// gracefully via rehash anyway.
    pub fn new_table_sized(&mut self, asize: usize) -> Gc<Table> {
        const MAX_ASIZE_HINT: usize = 1 << 27;
        let g = self.new_table();
        let clamped = asize.min(MAX_ASIZE_HINT);
        if clamped > 0 {
            // SAFETY: the freshly adopted table has no live borrow
            // anywhere else; we hold the only `Gc<Table>` handle.
            unsafe { g.as_mut() }.resize(self, clamped, 0);
        }
        g
    }

    /// Adopt a compiler-built prototype (its `hdr` must carry ObjTag::Proto).
    pub fn adopt_proto(&mut self, proto: Proto) -> Gc<Proto> {
        debug_assert!(proto.hdr.tag == ObjTag::Proto);
        self.adopt(Box::new(proto))
    }

    /// P11-S5d.M — back-compat constructor for callers that already
    /// built a `Box<[Gc<Upvalue>]>`. Internally re-routes through
    /// `new_closure_inline` so small-upval cases also pick the
    /// inline path (the input Box is freed after the copy).
    pub fn new_closure(&mut self, proto: Gc<Proto>, upvals: Box<[Gc<Upvalue>]>) -> Gc<LuaClosure> {
        use crate::runtime::function::INLINE_UPVALS_N;
        let n = upvals.len();
        if n <= INLINE_UPVALS_N {
            let g = self.new_closure_inline(proto, &upvals);
            drop(upvals);
            g
        } else {
            // Large closure — store the input Box directly in
            // `overflow`, no copy.
            self.adopt_closure_with(proto, n as u32, |c| {
                c.overflow = upvals;
            })
        }
    }

    /// P11-S5d.M — hot-path constructor for the `Op::Closure` handler.
    /// Takes a slice (typically backed by a stack array) so the caller
    /// doesn't allocate a Vec/Box just to hand it over. Upvals are
    /// copied into `inline_storage` for small closures, or into a
    /// freshly-allocated `Box<[..]>` for the rare overflow case.
    pub fn new_closure_inline(
        &mut self,
        proto: Gc<Proto>,
        upvals: &[Gc<Upvalue>],
    ) -> Gc<LuaClosure> {
        use crate::runtime::function::INLINE_UPVALS_N;
        let n = upvals.len();
        self.adopt_closure_with(proto, n as u32, |c| {
            if n <= INLINE_UPVALS_N {
                for (i, &uv) in upvals.iter().enumerate() {
                    c.inline_storage[i] = std::mem::MaybeUninit::new(uv);
                }
            } else {
                c.overflow = upvals.to_vec().into_boxed_slice();
            }
        })
    }

    fn adopt_closure_with<F: FnOnce(&mut LuaClosure)>(
        &mut self,
        proto: Gc<Proto>,
        upvals_len: u32,
        fill: F,
    ) -> Gc<LuaClosure> {
        let mut boxed = Box::new(LuaClosure {
            hdr: GcHeader::new(ObjTag::Closure),
            proto,
            upvals_ptr: std::ptr::null_mut(),
            upvals_len,
            inline_storage: [std::mem::MaybeUninit::<Gc<Upvalue>>::uninit();
                crate::runtime::function::INLINE_UPVALS_N],
            overflow: Box::new([]),
        });
        // Box is heap-stable now — populate storage at the final
        // address so `upvals_ptr` will be valid.
        fill(&mut boxed);
        let g = self.adopt(boxed);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { g.as_mut() }.init_upvals_ptr();
        g
    }

    /// Allocate a [`NativeClosure`] wrapping host function `f` with the
    /// given captured upvalues.
    pub fn new_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Gc<NativeClosure> {
        self.adopt(Box::new(NativeClosure {
            hdr: GcHeader::new(ObjTag::Native),
            f,
            upvals,
            is_async: false,
        }))
    }

    /// v1.1 B10 Stage 2 — like [`Heap::new_native`] but tags the
    /// closure with `is_async = true`. The dispatcher's native-call
    /// path then transmutes `f` to `AsyncNativeFn` and routes through
    /// the cooperative-yield path. The caller is responsible for
    /// having transmuted the `AsyncNativeFn` pointer to `NativeFn`
    /// shape (both are `fn` pointers of the same size); see
    /// [`crate::vm::async_drive`] for the helper that does this.
    pub fn new_async_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Gc<NativeClosure> {
        self.adopt(Box::new(NativeClosure {
            hdr: GcHeader::new(ObjTag::Native),
            f,
            upvals,
            is_async: true,
        }))
    }

    /// Allocate a fresh [`Upvalue`] cell in the given `state` (open / closed).
    pub fn new_upvalue(&mut self, state: UpvalState) -> Gc<Upvalue> {
        self.adopt(Box::new(Upvalue {
            hdr: GcHeader::new(ObjTag::Upvalue),
            state,
        }))
    }

    /// Create a fresh suspended coroutine wrapping `body`. The new thread
    /// inherits the creator's globals table; a `setfenv(0, env)` inside it
    /// will retune that copy without affecting the creator.
    pub fn new_coro(
        &mut self,
        body: Value,
        globals: Gc<crate::runtime::Table>,
    ) -> Gc<crate::runtime::Coro> {
        self.adopt(Box::new(crate::runtime::Coro {
            hdr: GcHeader::new(ObjTag::Coro),
            status: crate::runtime::CoroStatus::Suspended,
            body,
            started: false,
            resumer: None,
            resume_at: None,
            error_value: None,
            error_traceback: None,
            stack: Vec::new(),
            frames: Vec::new(),
            open_upvals: Vec::new(),
            tbc: Vec::new(),
            top: 0,
            pcall_depth: 0,
            hook: crate::vm::exec::HookState::default(),
            globals,
        }))
    }

    /// Create a userdata (an io file handle — luna's only userdata) with no
    /// metatable yet; the io library installs the shared `FILE*` metatable.
    pub fn new_userdata(&mut self, payload: UserdataPayload, writable: bool) -> Gc<Userdata> {
        self.adopt(Box::new(Userdata::new(
            GcHeader::new(ObjTag::Userdata),
            payload,
            writable,
        )))
    }

    /// Create (or find) a string. Short strings (≤ 40 bytes) are interned.
    pub fn intern(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        if bytes.len() <= string::MAX_SHORT_LEN {
            let (p, is_new) = self.strings.intern(bytes, self.seed);
            if is_new {
                self.link(p as *mut GcHeader);
                self.bytes += string::alloc_size(bytes.len());
            } else {
                // PUC `luaS_new` resurrect guard (lstring.c).
                // The bucket-chain is walked open-loop without consulting GC
                // color; during incremental sweep an existing entry may be
                // dead-white (in `sweep_cur`, scheduled for `free_obj`). If we
                // hand its pointer back, the budget-paced sweep frees it out
                // from under the mutator and the next bucket walk dereferences
                // a libc-recycled slot — the symptom recorded in
                // `.dev/known-bugs/stringtable-intern-uaf.md` (misaligned ptr
                // `0x800002a80000002d` deep in `StringTable::intern`).
                //
                // Flip the white bits to `current_white` so the upcoming sweep
                // skips it (PUC `changewhite`). Black / not-white objects are
                // already safe and untouched.
                // SAFETY: `p` came from `StringTable::intern` and is a valid
                // `LuaStr` header (its bucket chain is consistent under our
                // single-threaded heap).
                unsafe {
                    let f = (*(p as *mut GcHeader)).flags;
                    if is_white(f) && (f & self.current_white) == 0 {
                        (*(p as *mut GcHeader)).flags = (f & !WHITE_BITS) | self.current_white;
                    }
                }
            }
            Gc::from_ptr(p)
        } else {
            let p = string::alloc_long(bytes, self.seed);
            self.link(p as *mut GcHeader);
            self.bytes += string::alloc_size(bytes.len());
            Gc::from_ptr(p)
        }
    }

    /// Number of GC-managed objects currently linked into the heap (live + not
    /// yet swept). Useful for `collectgarbage("count")`-style introspection.
    pub fn live_objects(&self) -> usize {
        self.live
    }

    /// Approximate heap size in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// v2.0 Track TL — pure-read walk over the intrusive `all`
    /// objects list, invoking `visit(tag)` once per live (or
    /// not-yet-swept) GC-managed object. Used by `luna-tools`'s
    /// `luna-heap-dump` to build a per-type histogram; embedders
    /// can reuse it for ad-hoc heap introspection.
    ///
    /// # Read-only contract
    ///
    /// The callback receives only the [`ObjTag`] discriminant and
    /// is invoked under a `&self` borrow on the heap: no pointer
    /// to the GC payload escapes, no `as_mut`-style aliasing is
    /// available, and the walk performs zero allocation in the
    /// loop. Safe to call between dispatch ticks (the only allocs
    /// happen in the caller's bookkeeping).
    ///
    /// The walk visits both the live `all` list and the
    /// `sweep_cur` detached list so a mid-cycle invocation reports
    /// the same total as [`Heap::live_objects`].
    pub fn walk_objects(&self, mut visit: impl FnMut(ObjTag)) {
        for head in [self.all, self.sweep_cur] {
            let mut cur = head;
            while !cur.is_null() {
                // SAFETY: pointers come from the runtime's
                // intrusive all-objects list. `&self` borrow on
                // the heap prevents concurrent mutation; the GC
                // cannot run while this walk holds the borrow,
                // so every `next` link is valid until consumed.
                let (tag, next) = unsafe { ((*cur).tag, (*cur).next) };
                visit(tag);
                cur = next;
            }
        }
    }

    /// Whether allocation has crossed the auto-GC threshold (cheap safe-point
    /// check for the interpreter loop).
    #[inline(always)]
    pub fn gc_due(&self) -> bool {
        !self.gc_stopped && self.bytes >= self.next_gc
    }

    /// `collectgarbage("stop"/"restart")`: suspend or resume auto-GC.
    pub(crate) fn gc_is_stopped(&self) -> bool {
        self.gc_stopped
    }

    pub(crate) fn gc_set_stopped(&mut self, stopped: bool) {
        self.gc_stopped = stopped;
    }

    /// Re-arm with caller-supplied `pause` (PUC param, % of live bytes). The
    /// next cycle fires once `bytes >= live * pause / 100`. `pause=200` (PUC
    /// default) waits for the heap to double; `pause=100` fires immediately
    /// when alloc resumes; `pause=300` is 3× — lower pause = more aggressive.
    pub fn rearm_gc_pause(&mut self, pause: i64) {
        let pause = pause.max(0) as usize;
        let target = self
            .bytes
            .saturating_mul(pause)
            .saturating_div(100)
            .max(GC_MIN_THRESHOLD);
        self.next_gc = target;
    }

    /// Re-arm the auto-GC threshold after a collection (PUC pause-style: next
    /// collection once the live set roughly doubles).
    pub fn rearm_gc(&mut self) {
        self.next_gc = self.bytes.saturating_mul(2).max(GC_MIN_THRESHOLD);
    }

    /// Apply a `before → after` box-size delta from a Table mutation
    /// (`set`/`rehash`/`ensure_*`). Grows credit `Heap.bytes`; shrinks
    /// debit it. `free_obj` for `ObjTag::Table` then subtracts the table's
    /// final `internal_bytes()` so the round-trip is symmetric across the
    /// table's whole lifetime.
    pub(crate) fn apply_bytes_delta(&mut self, before: usize, after: usize) {
        if after > before {
            self.bytes += after - before;
        } else if before > after {
            self.bytes = self.bytes.saturating_sub(before - after);
        }
    }

    /// Mark from `roots`, sweep everything unreachable. Returns the number of
    /// objects freed.
    pub fn collect(&mut self, roots: &[Value]) -> usize {
        self.collect_ex(roots, &[])
    }

    /// Like `collect`, with additional bare-object roots (e.g. the VM's open
    /// upvalues, which are not first-class Values).
    pub(crate) fn collect_ex(&mut self, roots: &[Value], extra: &[*mut GcHeader]) -> usize {
        // a full STW collection subsumes any in-flight incremental cycle:
        // drive it to completion (Propagate → atomic → Sweep → Pause) so `all`
        // holds the whole heap again with all marks cleared, then run a fresh
        // STW cycle. Any tobefnz from the finished cycle stays queued and is
        // re-marked (kept alive) by the upcoming mark_all so the VM's
        // run_finalizers can still see them.
        if self.phase == GcPhase::Propagate {
            self.gc_finish_atomic();
        }
        if self.phase == GcPhase::Sweep {
            self.gc_sweep_step(usize::MAX);
        }
        self.mark_all(roots, extra);
        self.full_sweep()
    }

    /// Stop-the-world mark from `roots`/`extra`. Builds an ephemeral marker,
    /// seeds from roots + extra + tobefnz + any barrier-carried gray queue,
    /// propagates to completion, then runs the atomic tail (weak / ephemeron
    /// / finalizer resurrection / current-white flip). After return all
    /// reachable objects are BLACK and `current_white` has flipped, so the
    /// caller's sweep tests `other-white` for dead. Does NOT change `phase`.
    fn mark_all(&mut self, roots: &[Value], extra: &[*mut GcHeader]) {
        let mut m = Marker {
            stack: Vec::new(),
            weak: Vec::new(),
            ephemeron: Vec::new(),
            no_ephemeron: self.no_ephemeron,
            cached_protos: Vec::new(),
        };
        // Drain any barrier-grayed objects carried over: each was demoted from
        // BLACK back to gray by a write barrier and is awaiting (re-)trace.
        m.stack.append(&mut self.gray);
        for &r in roots {
            m.value(r);
        }
        for &h in extra {
            m.header(h);
        }
        // objects already queued for finalization but not yet run must stay
        // alive until the VM calls their `__gc` (they may be unreachable now).
        for &h in &self.tobefnz {
            m.header(h);
        }
        drain_marker(&mut m);
        // ephemeron convergence: a weak-key entry's value is reachable only if
        // the key is. Marking a value can make another key reachable, so repeat
        // until no value is newly marked (PUC convergeephemerons).
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, &mut m) };
                }
                drain_marker(&mut m);
                if !changed {
                    break;
                }
            }
        }
        self.atomic_tail(&mut m);
    }

    /// PUC `atomic()` tail: weak-table value-clear, finalizer resurrection,
    /// post-resurrection ephemeron convergence, proto cache, key-clear, late
    /// value-clear, and current-white flip. Marker is consumed; `weak` is
    /// empty on return.
    ///
    /// Shared between the STW path (`mark_all`) and the incremental path
    /// (`gc_finish_atomic`). PUC 5.5 `lgc.c::atomic` mirror:
    ///   propagate → remarkupvals → convergeephemerons
    ///   → clearbyvalues(weak, NULL)            ─ early value-clear
    ///   → clearbyvalues(allweak, NULL)         ─ (same pass under luna)
    ///   → origweak = g->weak                   ─ snapshot pre-resurrection
    ///   → separatetobefnz(0) + markbeingfnz    ─ separate_finalizables
    ///   → propagateall + convergeephemerons    ─ post-resurrection
    ///   → clearbykeys(ephemeron) + clearbykeys(allweak)
    ///   → clearbyvalues(weak, origweak)        ─ NEW (post-resurrect) only
    ///   → clearbyvalues(allweak, origall)      ─ (same)
    /// The `origweak` split matters because finalizer resurrection can
    /// re-trace fresh proto/closure → new weak tables joining `m.weak`;
    /// PUC limits the late value-clear to those new heads.
    fn atomic_tail(&mut self, m: &mut Marker) {
        let early_is_dead = |v: Value| -> bool {
            let h = match v {
                Value::Str(_) => return false,
                Value::Table(t) => t.as_ptr() as *mut GcHeader,
                Value::Closure(c) => c.as_ptr() as *mut GcHeader,
                Value::Native(n) => n.as_ptr() as *mut GcHeader,
                Value::Coro(c) => c.as_ptr() as *mut GcHeader,
                Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
                _ => return false,
            };
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe { is_white((*h).flags) }
        };
        let mark_string = |v: Value| {
            if let Value::Str(s) = v {
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                unsafe {
                    let h = s.as_ptr() as *mut GcHeader;
                    // strings are leaves: skip gray and go straight to black
                    (*h).flags = ((*h).flags & !COLOR_BITS) | BLACK;
                }
            }
        };
        // (1) early clearbyvalues — drop dead-value entries from every weak
        // table on `m.weak` (PUC's combined `clearbyvalues(weak, NULL) +
        // clearbyvalues(allweak, NULL)`). Keys are deferred to the
        // post-resurrection sweep below.
        for t in &m.weak {
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe {
                let (_wk, wv) = (**t).weak_mode();
                if wv {
                    (**t).clear_weak(false, true, &early_is_dead, &mark_string);
                }
            }
        }
        // (2) `origweak` snapshot — PUC takes the list head; luna's `m.weak`
        // is a Vec, so the equivalent is its length before resurrection.
        // Anything appended past this index is a "NEW" weak table that the
        // resurrection pass brought into view.
        let origweak_n = m.weak.len();
        // (3) separate + markbeingfnz — resurrect every registered finalizable
        // that ended up unmarked. `m.header(h)` enqueues each into the marker
        // so the following drain_marker propagates through it.
        self.separate_finalizables(m);
        drain_marker(m);
        // (4) post-resurrection ephemeron convergence — a resurrected
        // finalizable may bring new keys into reach, which in turn marks new
        // ephemeron values.
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, m) };
                }
                drain_marker(m);
                if !changed {
                    break;
                }
            }
        }
        // (5) closure-cache weak refs — PUC `traverseproto` clears
        // `Proto.cache` when the cached LClosure ended the cycle unmarked.
        // Without this, an LClosure whose only outstanding reference is the
        // proto's cache would survive forever and its upvalues' `__gc`
        // finalisers would never run.
        for &p in &m.cached_protos {
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe {
                if let Some(c) = (*p).cache.get() {
                    let h = c.as_ptr() as *mut GcHeader;
                    if is_white((*h).flags) {
                        (*p).cache.set(None);
                    }
                }
            }
        }
        // (6) clearbykeys — drop entries whose weak key did not survive
        // marking, across every weak table (PUC's `clearbykeys(ephemeron)
        // + clearbykeys(allweak)`). Pure key sweep — value-dead entries are
        // either already nil from step (1) or wait for step (7).
        for t in &m.weak {
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe {
                let (wk, _wv) = (**t).weak_mode();
                if wk {
                    (**t).clear_weak(true, false, &early_is_dead, &mark_string);
                }
            }
        }
        // (7) late clearbyvalues — PUC's `clearbyvalues(weak, origweak) +
        // clearbyvalues(allweak, origall)`. Limit the sweep to NEW heads so
        // we don't redo work already done in step (1) for the pre-resurrect
        // tables (they were drained by then and re-marking happens through
        // mark_string in step (6)). resurrected weak tables joining `m.weak`
        // past `origweak_n` get their first value-clear here.
        let weak_snapshot = std::mem::take(&mut m.weak);
        for t in &weak_snapshot[origweak_n..] {
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe {
                let (_wk, wv) = (**t).weak_mode();
                if wv {
                    (**t).clear_weak(false, true, &early_is_dead, &mark_string);
                }
            }
        }
        // PUC 5.5 `atomic` end: flip currentwhite so survivors (presently
        // BLACK) get transitioned into the *new* current-white during sweep,
        // and the pre-flip current-white becomes the dead-white (the bit the
        // sweep tests for). Born-during-sweep allocations stamp the new
        // current-white via `Heap::link`, so they survive this cycle.
        self.current_white ^= WHITE_BITS;
    }

    /// Borrow Heap's persistent propagate state as an ephemeral Marker.
    /// Caller MUST call `stash_marker` with the same Marker after work to
    /// write the (potentially mutated) state back. Used by the incremental
    /// Propagate path to avoid lifetime entanglement between `&mut self` and
    /// `&mut self.propagate`.
    fn loan_marker(&mut self) -> Marker {
        let mut prop = self
            .propagate
            .take()
            .expect("propagate state taken outside Propagate phase");
        Marker {
            stack: std::mem::take(&mut self.gray),
            weak: std::mem::take(&mut prop.weak),
            ephemeron: std::mem::take(&mut prop.ephemeron),
            no_ephemeron: prop.no_ephemeron,
            cached_protos: std::mem::take(&mut prop.cached_protos),
        }
    }

    fn stash_marker(&mut self, m: Marker) {
        let no_ephemeron = m.no_ephemeron;
        self.gray = m.stack;
        self.propagate = Some(PropagateState {
            weak: m.weak,
            ephemeron: m.ephemeron,
            cached_protos: m.cached_protos,
            no_ephemeron,
        });
    }

    /// Begin an incremental mark cycle: seed the persistent gray queue from
    /// roots + extra + tobefnz + any barrier-carried gray, install a fresh
    /// PropagateState, and enter `GcPhase::Propagate`. Precondition: `Pause`.
    pub(crate) fn gc_start_propagate(&mut self, roots: &[Value], extra: &[*mut GcHeader]) {
        debug_assert!(self.phase == GcPhase::Pause);
        self.phase = GcPhase::Propagate;
        self.propagate = Some(PropagateState {
            weak: Vec::new(),
            ephemeron: Vec::new(),
            cached_protos: Vec::new(),
            no_ephemeron: self.no_ephemeron,
        });
        let mut m = self.loan_marker();
        for &r in roots {
            m.value(r);
        }
        for &h in extra {
            m.header(h);
        }
        for &h in &self.tobefnz {
            m.header(h);
        }
        self.stash_marker(m);
    }

    /// Drain up to `budget` gray objects (blacken + trace). Returns true if
    /// the gray queue is now empty (caller should follow up with
    /// `gc_finish_atomic`). PUC `propagatemark` budgeted loop.
    pub(crate) fn gc_step_propagate(&mut self, budget: usize) -> bool {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        let mut n = 0;
        while n < budget {
            let Some(h) = m.stack.pop() else {
                break;
            };
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe {
                (*h).flags = ((*h).flags & !WHITE_BITS) | BLACK;
                match (*h).tag {
                    ObjTag::Str => {}
                    ObjTag::Table => (*(h as *mut Table)).trace(&mut m),
                    ObjTag::Proto => (*(h as *mut Proto)).trace(&mut m),
                    ObjTag::Closure => (*(h as *mut LuaClosure)).trace(&mut m),
                    ObjTag::Upvalue => (*(h as *mut Upvalue)).trace(&mut m),
                    ObjTag::Native => (*(h as *mut NativeClosure)).trace(&mut m),
                    ObjTag::Coro => (*(h as *mut crate::runtime::Coro)).trace(&mut m),
                    ObjTag::Userdata => (*(h as *mut Userdata)).trace(&mut m),
                }
            }
            n += 1;
        }
        let exhausted = m.stack.is_empty();
        self.stash_marker(m);
        exhausted
    }

    /// Conclude a Propagate cycle: drain any residual gray, run the atomic
    /// tail (weak / ephemeron / finalizer / proto-cache / flip), detach `all`
    /// into `sweep_cur`, and enter `GcPhase::Sweep`. Releases `propagate`.
    /// PUC `atomic` + `entersweep` transition.
    pub(crate) fn gc_finish_atomic(&mut self) {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        // any residual gray (caller may not have drained to empty)
        drain_marker(&mut m);
        // pre-atomic ephemeron convergence
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, &mut m) };
                }
                drain_marker(&mut m);
                if !changed {
                    break;
                }
            }
        }
        self.atomic_tail(&mut m);
        // PropagateState consumed; transition to Sweep phase by detaching
        // the whole heap into sweep_cur (mirrors gc_mark_atomic). Anything
        // allocated past this point links onto fresh `all` and survives.
        self.propagate = None;
        debug_assert!(self.gray.is_empty(), "gray queue not drained at atomic");
        self.sweep_cur = std::mem::replace(&mut self.all, ptr::null_mut());
        self.phase = GcPhase::Sweep;
    }

    /// Phase peek (for the VM-side step driver).
    pub(crate) fn gc_phase_is_pause(&self) -> bool {
        self.phase == GcPhase::Pause
    }
    pub(crate) fn gc_phase_is_propagate(&self) -> bool {
        self.phase == GcPhase::Propagate
    }
    #[allow(dead_code)] // public phase-peek API trio; sweep variant unused internally
    pub(crate) fn gc_phase_is_sweep(&self) -> bool {
        self.phase == GcPhase::Sweep
    }

    /// Forward write barrier: when a BLACK `parent` acquires a fresh reference
    /// to a WHITE `child`, gray the child (strings go straight to BLACK as
    /// leaves) and push onto the persistent gray queue so the next propagate
    /// step traces it. Mirrors PUC `luaC_barrier_`. No-op outside Propagate
    /// (parent is gray or white — the mutator never sees a BLACK object live
    /// outside an incremental cycle).
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // Internal GC barrier; caller (Gc<T>::write_*) guarantees ptr validity per SAFETY below.
    pub fn barrier_forward(&mut self, parent: *mut GcHeader, child: Value) {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            if !is_black((*parent).flags) {
                return;
            }
            let ch = match child {
                Value::Str(s) => s.as_ptr() as *mut GcHeader,
                Value::Table(t) => t.as_ptr() as *mut GcHeader,
                Value::Closure(c) => c.as_ptr() as *mut GcHeader,
                Value::Native(n) => n.as_ptr() as *mut GcHeader,
                Value::Coro(c) => c.as_ptr() as *mut GcHeader,
                Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
                _ => return,
            };
            let cf = (*ch).flags;
            if !is_white(cf) {
                return;
            }
            if (*ch).tag == ObjTag::Str {
                (*ch).flags = (cf & !COLOR_BITS) | BLACK;
            } else {
                (*ch).flags = cf & !WHITE_BITS;
                self.gray.push(ch);
            }
        }
    }

    /// Backward write barrier for objects with many fields (tables, threads):
    /// demote the parent itself back to gray so propagate re-traces it.
    /// Mirrors PUC `luaC_barrierback_`. One call covers any number of
    /// subsequent stores until the next propagate finishes — much cheaper for
    /// tables than per-child forward barriers. No-op outside Propagate.
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // Internal GC barrier; caller (Gc<T>::write_*) guarantees ptr validity per SAFETY below.
    pub fn barrier_back(&mut self, parent: *mut GcHeader) {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            let f = (*parent).flags;
            if !is_black(f) {
                return;
            }
            (*parent).flags = f & !COLOR_BITS;
            self.gray.push(parent);
        }
    }

    /// Move every registered finalizable that is now unreachable to `tobefnz`
    /// and resurrect it (mark it via `m`) so it — and the data its `__gc` needs
    /// — survives this cycle. Survivors stay registered in `finalize`. PUC's
    /// `separatetobefnz` walks `g->finobj` head-first, but `g->finobj` is a
    /// linked list that registration *prepends* to — so dead objects end up
    /// in `tobefnz` in reverse registration order, and `__gc` ultimately
    /// runs LIFO. luna's `finalize` is a Vec that grows forward, so iterate
    /// it in reverse here to match the LIFO contract (gc.lua's userdata
    /// section asserts the finalizers fire from value 10 back to 0).
    fn separate_finalizables(&mut self, m: &mut Marker) {
        let mut i = self.finalize.len();
        while i > 0 {
            i -= 1;
            let h = self.finalize[i];
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            if unsafe { is_white((*h).flags) } {
                // Two-pass cycle-finalize (PUC 5.3 gc.lua :502): when a
                // finalizable table holds onto an unreachable coroutine, the
                // cycle (table → coroutine.stack → closure → table) keeps the
                // mark phase from reaching the table even though it is still
                // logically alive for one more GC pass. PUC's mark-sweep wakes
                // it via `markbeingfnz` *after* sweeping, so the actual `__gc`
                // call lands one cycle later. luna mirrors this by resurrecting
                // the table on the first sighting and only enqueuing it for
                // `__gc` on the second.
                let in_thread_cycle = self.defer_thread_cycle_finalize
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    && unsafe { (*h).tag } == ObjTag::Table
                    && {
                        let t = h as *mut Table;
                        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                        unsafe { (*t).refs_contain_unmarked_coro() }
                    };
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                let already_deferred = unsafe { (*h).flags & DEFERRED != 0 };
                if in_thread_cycle && !already_deferred {
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    unsafe { (*h).flags |= DEFERRED };
                    m.header(h);
                    continue;
                }
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                unsafe { (*h).flags = ((*h).flags & !(FIN | DEFERRED)) | FINALIZED };
                self.tobefnz.push(h);
                m.header(h);
                self.finalize.swap_remove(i);
            } else {
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                unsafe { (*h).flags &= !DEFERRED };
            }
        }
    }

    /// Register a table for finalization (a live `__gc` metamethod was just set
    /// via setmetatable). No-op if it is already pending a finalize (FIN bit).
    /// PUC 5.5 reference manual §2.5.3: "An object can be marked again for
    /// finalization by calling setmetatable with a different metatable, or
    /// with the same metatable but with a different __gc field" — so a
    /// previously finalized object (FINALIZED bit set then reset by
    /// `take_tobefnz`) can re-register. PUC's `luaC_checkfinalizer` is gated
    /// on `tofinalize(o)` only, which mirrors checking the FIN bit.
    pub(crate) fn register_finalizable(&mut self, t: Gc<Table>) {
        let h = t.as_ptr() as *mut GcHeader;
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            if (*h).flags & FIN == 0 {
                (*h).flags |= FIN;
                self.finalize.push(h);
            }
        }
    }

    /// Register a userdata for finalization. PUC 5.1 `newproxy(true)` plus a
    /// metatable carrying `__gc` lets a Lua script attach a finalizer to a
    /// proxy object — gc.lua's "testing userdata" section binds this
    /// behaviour together with weak tables.
    pub(crate) fn register_finalizable_userdata(&mut self, u: Gc<crate::runtime::Userdata>) {
        let h = u.as_ptr() as *mut GcHeader;
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            if (*h).flags & FIN == 0 {
                (*h).flags |= FIN;
                self.finalize.push(h);
            }
        }
    }

    /// Take the objects awaiting their `__gc` call (the VM runs the
    /// finalizers). Each entry is dispatched on its `ObjTag` so the caller
    /// can look up `__gc` for either a table or a proxy userdata.
    ///
    /// Mirrors PUC 5.5 `udata2finalize`: the FINALIZED bit is reset on the
    /// way out so a `setmetatable(obj, mt_with___gc)` inside (or after) the
    /// finalizer can re-register the object for a future round, per the Lua
    /// 5.5 reference manual §2.5.3 re-finalize semantics.
    pub(crate) fn take_tobefnz(&mut self) -> Vec<crate::runtime::Value> {
        use crate::runtime::Value;
        std::mem::take(&mut self.tobefnz)
            .into_iter()
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            .map(|h| unsafe {
                (*h).flags &= !FINALIZED;
                match (*h).tag {
                    ObjTag::Table => Value::Table(Gc::from_ptr(h as *mut Table)),
                    ObjTag::Userdata => {
                        Value::Userdata(Gc::from_ptr(h as *mut crate::runtime::Userdata))
                    }
                    _ => unreachable!("non-finalizable object queued for finalization"),
                }
            })
            .collect()
    }

    /// Move ALL still-registered finalizables to the pending queue regardless of
    /// reachability (PUC separatetobefnz(g, 1) at state close), so the VM can run
    /// every `__gc` before the heap is torn down.
    pub(crate) fn queue_all_finalizers(&mut self) {
        for h in std::mem::take(&mut self.finalize) {
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe { (*h).flags = ((*h).flags & !FIN) | FINALIZED };
            self.tobefnz.push(h);
        }
    }

    /// Sweep the whole `all` list in one pass: free dead-white objects,
    /// transition survivors (BLACK or current-white) to the new current-white
    /// so the next cycle can re-mark them. Returns the number of objects freed.
    fn full_sweep(&mut self) -> usize {
        // detach the list first so freeing (which needs &mut self for the
        // string table) never aliases a pointer into self
        let mut freed = 0;
        let new_white = self.current_white;
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            let mut cur = std::mem::replace(&mut self.all, ptr::null_mut());
            let mut kept_head: *mut GcHeader = ptr::null_mut();
            let mut kept_tail: *mut GcHeader = ptr::null_mut();
            while !cur.is_null() {
                let next = (*cur).next;
                let f = (*cur).flags;
                // dead = other-white (i.e. white but not current-white).
                // Survivors are BLACK (just-marked) or current-white (born
                // during the sweep itself).
                let dead = is_white(f) && (f & new_white) == 0;
                if !dead {
                    (*cur).flags = (f & !COLOR_BITS) | new_white;
                    (*cur).next = ptr::null_mut();
                    if kept_tail.is_null() {
                        kept_head = cur;
                    } else {
                        (*kept_tail).next = cur;
                    }
                    kept_tail = cur;
                } else {
                    self.free_obj(cur);
                    freed += 1;
                }
                cur = next;
            }
            self.all = kept_head;
        }
        self.live -= freed;
        freed
    }

    /// Sweep up to `budget` objects from the detached `sweep_cur` list: free
    /// unmarked ones, splice marked survivors back onto `all` (clearing their
    /// MARK bit). Returns true once the list is exhausted (cycle complete →
    /// back to `Pause`). Safe with no write barrier: marking was atomic, so any
    /// object still unmarked here was unreachable at mark time and the mutator
    /// holds no reference that could have resurrected it.
    pub(crate) fn gc_sweep_step(&mut self, budget: usize) -> bool {
        let mut n = 0;
        let new_white = self.current_white;
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            while n < budget && !self.sweep_cur.is_null() {
                let cur = self.sweep_cur;
                let next = (*cur).next;
                let f = (*cur).flags;
                let dead = is_white(f) && (f & new_white) == 0;
                if !dead {
                    (*cur).flags = (f & !COLOR_BITS) | new_white;
                    (*cur).next = self.all;
                    self.all = cur;
                } else {
                    self.free_obj(cur);
                    self.live -= 1;
                }
                self.sweep_cur = next;
                n += 1;
            }
        }
        if self.sweep_cur.is_null() {
            self.phase = GcPhase::Pause;
            true
        } else {
            false
        }
    }

    unsafe fn free_obj(&mut self, h: *mut GcHeader) {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            match (*h).tag {
                ObjTag::Table => {
                    let t = h as *mut Table;
                    let internal = (*t).internal_bytes();
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<Table>() + internal);
                    // P17-D v2 layer-15 attack — pool recycle. Drop the
                    // Box-owned interior (slab, nodes, metatable) so the
                    // Table struct itself can be re-handed-out by a
                    // future `new_table` without re-mallocing. Cap pool
                    // at 4096 entries to bound idle memory.
                    const TABLE_POOL_CAP: usize = 4096;
                    if self.table_pool.len() < TABLE_POOL_CAP {
                        // Free interior heap allocations now (slab + nodes).
                        // Each Box::new([]) is a dangling no-alloc empty
                        // slice, so reassigning is just a pointer move.
                        (*t).slab = Box::new([]);
                        (*t).nodes = Box::new([]);
                        // C3 — drop SoA Robin Hood parallel arrays too.
                        // These are Box::new([]) dangling stubs until
                        // Phase D cuts over (Phase B initial state).
                        (*t).keys = Box::new([]);
                        (*t).vals = Box::new([]);
                        (*t).meta = Box::new([]);
                        (*t).tombstones = 0;
                        (*t).iter_depth = 0;
                        (*t).metatable = None;
                        // Stash the raw pointer for future reuse.
                        // SAFETY: t is non-null (came from a live Gc<Table>);
                        // pool owns it until reuse or Heap::Drop.
                        self.table_pool.push(std::ptr::NonNull::new_unchecked(t));
                    } else {
                        drop(Box::from_raw(t));
                    }
                }
                ObjTag::Proto => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<Proto>());
                    drop(Box::from_raw(h as *mut Proto));
                }
                ObjTag::Closure => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<LuaClosure>());
                    drop(Box::from_raw(h as *mut LuaClosure));
                }
                ObjTag::Upvalue => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<Upvalue>());
                    drop(Box::from_raw(h as *mut Upvalue));
                }
                ObjTag::Native => {
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<NativeClosure>());
                    drop(Box::from_raw(h as *mut NativeClosure));
                }
                ObjTag::Coro => {
                    self.bytes = self
                        .bytes
                        .saturating_sub(std::mem::size_of::<crate::runtime::Coro>());
                    drop(Box::from_raw(h as *mut crate::runtime::Coro));
                }
                ObjTag::Userdata => {
                    self.bytes = self.bytes.saturating_sub(std::mem::size_of::<Userdata>());
                    drop(Box::from_raw(h as *mut Userdata));
                }
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    self.bytes = self.bytes.saturating_sub(string::alloc_size((*s).len()));
                    if (*s).is_short() {
                        self.strings.remove(s);
                    }
                    string::free(s);
                }
            }
        }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // free everything regardless of reachability, including any list still
        // detached for an in-flight incremental sweep
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            for mut cur in [self.all, self.sweep_cur] {
                while !cur.is_null() {
                    let next = (*cur).next;
                    self.free_obj(cur);
                    cur = next;
                }
            }
            // P17-D v2 layer-15 attack — release the table_pool's
            // dangling Box<Table> ptrs. Each was Box::into_raw'd into
            // the pool (via free_obj recycle path); without this, the
            // Tables would leak. The pool's Tables had their interior
            // Box-owned fields (slab/nodes/metatable) already cleared
            // when they were recycled, so dropping the Table now only
            // releases the Table struct itself.
            for ptr in self.table_pool.drain(..) {
                drop(Box::from_raw(ptr.as_ptr()));
            }
        }
    }
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

/// Mark accumulator: gray stack plus entry points for Values and bare
/// object headers (Protos/Upvalues are not first-class Values).
pub(crate) struct Marker {
    stack: Vec<*mut GcHeader>,
    /// live tables with a weak `__mode`, collected during marking and processed
    /// (dead weak entries cleared) before the sweep
    pub(crate) weak: Vec<*mut Table>,
    /// ephemeron tables (weak keys, strong values): their hash values are not
    /// marked during trace but in a fixpoint pass keyed on key-reachability
    pub(crate) ephemeron: Vec<*mut Table>,
    /// PUC 5.1 mode: skip ephemeron handling — `__mode='k'` tables mark their
    /// values strongly during the normal trace pass (see [`Heap::no_ephemeron`]).
    pub(crate) no_ephemeron: bool,
    /// Protos with a non-null closure cache (PUC `Proto.cache`). After
    /// marking is done, any cached LClosure that ended the cycle unmarked is
    /// cleared so the sweep can collect it — the cache is a *weak* reference
    /// (PUC `traverseproto` checks `iswhite(cache)`). Seen via [`Proto::trace`].
    pub(crate) cached_protos: Vec<*mut crate::runtime::Proto>,
}

/// Drain the gray stack: pop each marked object and trace its children until
/// the worklist is empty (iterative, so deep graphs don't overflow the Rust
/// stack). Shared by the root mark and the post-resurrection remark.
fn drain_marker(m: &mut Marker) {
    while let Some(h) = m.stack.pop() {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            // PUC `propagatemark`: gray → black before scanning children, so a
            // child that points back at us (cycle) re-traces us as already
            // black and does not loop. White bits were cleared on push.
            (*h).flags = ((*h).flags & !WHITE_BITS) | BLACK;
            match (*h).tag {
                ObjTag::Str => {}
                ObjTag::Table => (*(h as *mut Table)).trace(m),
                ObjTag::Proto => (*(h as *mut Proto)).trace(m),
                ObjTag::Closure => (*(h as *mut LuaClosure)).trace(m),
                ObjTag::Upvalue => (*(h as *mut Upvalue)).trace(m),
                ObjTag::Native => (*(h as *mut NativeClosure)).trace(m),
                ObjTag::Coro => (*(h as *mut crate::runtime::Coro)).trace(m),
                ObjTag::Userdata => (*(h as *mut Userdata)).trace(m),
            }
        }
    }
}

impl Marker {
    /// Mark a value, returning true if it was newly marked (was white).
    pub(crate) fn value(&mut self, v: Value) -> bool {
        let h = match v {
            Value::Str(s) => s.as_ptr() as *mut GcHeader,
            Value::Table(t) => t.as_ptr() as *mut GcHeader,
            Value::Closure(c) => c.as_ptr() as *mut GcHeader,
            Value::Native(n) => n.as_ptr() as *mut GcHeader,
            Value::Coro(c) => c.as_ptr() as *mut GcHeader,
            Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
            _ => return false,
        };
        self.header(h)
    }

    /// Mark a bare header, returning true if it was newly marked (was white).
    /// Transitions white → gray (in PUC `reallymarkobject` terms): clears the
    /// current-white bit and pushes onto the gray stack. `drain_marker` later
    /// pops it, traces children, and stamps it BLACK.
    pub(crate) fn header(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            let f = (*h).flags;
            if is_white(f) {
                (*h).flags = f & !WHITE_BITS;
                self.stack.push(h);
                true
            } else {
                false
            }
        }
    }
}

/// Whether a value is "alive" for ephemeron key purposes: non-collectable
/// values and strings are always alive (strings are never weakly cleared);
/// a collectable object is alive only once marked (gray or black).
fn weak_key_alive(v: Value) -> bool {
    let h = match v {
        Value::Table(t) => t.as_ptr() as *mut GcHeader,
        Value::Closure(c) => c.as_ptr() as *mut GcHeader,
        Value::Native(n) => n.as_ptr() as *mut GcHeader,
        Value::Coro(c) => c.as_ptr() as *mut GcHeader,
        Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
        _ => return true, // strings, numbers, booleans: never weak-collected
    };
    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
    unsafe { !is_white((*h).flags) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::isa::{Inst, Op};

    #[test]
    fn collect_traces_function_objects() {
        let mut heap = Heap::new();
        let source = heap.intern(b"@test");
        let kstr = heap.intern(b"a-constant-string");
        let inner = Proto {
            hdr: GcHeader::new(ObjTag::Proto),
            code: Box::new([Inst::iabc(Op::Return0, 0, 0, 0, false)]),
            consts: Box::new([]),
            protos: Box::new([]),
            upvals: Box::new([]),
            num_params: 0,
            is_vararg: false,
            has_vararg_table_pseudo: false,
            has_compat_vararg_arg: false,
            max_stack: 2,
            lines: Box::new([1]),
            source,
            line_defined: 1,
            last_line_defined: 1,
            locvars: Box::new([]),
            cache: std::cell::Cell::new(None),
            jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
            env_upval_idx: u8::MAX,
            trace_hot_count: std::cell::Cell::new(0),
            call_hot_count: std::cell::Cell::new(0),
            trace_discard_count: std::cell::Cell::new(0),
            trace_gave_up: std::cell::Cell::new(false),
            traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        };
        let inner = heap.adopt_proto(inner);
        let outer = Proto {
            hdr: GcHeader::new(ObjTag::Proto),
            code: Box::new([Inst::iabc(Op::Return0, 0, 0, 0, false)]),
            consts: Box::new([Value::Str(kstr)]),
            protos: Box::new([inner]),
            upvals: Box::new([]),
            num_params: 0,
            is_vararg: true,
            has_vararg_table_pseudo: false,
            has_compat_vararg_arg: false,
            max_stack: 2,
            lines: Box::new([1]),
            source,
            line_defined: 0,
            last_line_defined: 0,
            locvars: Box::new([]),
            cache: std::cell::Cell::new(None),
            jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
            env_upval_idx: u8::MAX,
            trace_hot_count: std::cell::Cell::new(0),
            call_hot_count: std::cell::Cell::new(0),
            trace_discard_count: std::cell::Cell::new(0),
            trace_gave_up: std::cell::Cell::new(false),
            traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        };
        let outer = heap.adopt_proto(outer);
        let captured = heap.intern(b"captured-value-string-xxxxxxxxxxxxxxxxxxxxxxxxx");
        let uv = heap.new_upvalue(UpvalState::Closed(Value::Str(captured)));
        let cl = heap.new_closure(outer, Box::new([uv]));
        // objects: source, kstr, inner, outer, captured, uv, cl
        assert_eq!(heap.live_objects(), 7);
        // rooting the closure keeps the whole graph alive
        assert_eq!(heap.collect(&[Value::Closure(cl)]), 0);
        assert_eq!(heap.live_objects(), 7);
        assert_eq!(heap.collect(&[]), 7);
        assert_eq!(heap.live_objects(), 0);
    }

    #[test]
    fn collect_unreachable() {
        let mut heap = Heap::new();
        let s = heap.intern(b"hello");
        let t = heap.new_table();
        assert_eq!(heap.live_objects(), 2);
        // both rooted: nothing freed
        assert_eq!(heap.collect(&[Value::Str(s), Value::Table(t)]), 0);
        // only table rooted: string freed
        assert_eq!(heap.collect(&[Value::Table(t)]), 1);
        assert_eq!(heap.live_objects(), 1);
        // nothing rooted
        assert_eq!(heap.collect(&[]), 1);
        assert_eq!(heap.live_objects(), 0);
    }

    #[test]
    fn collect_traces_table_contents() {
        let mut heap = Heap::new();
        let t = heap.new_table();
        let k = heap.intern(b"key-string-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // long
        let v = heap.intern(b"val");
        unsafe { t.as_mut() }
            .set(&mut heap, Value::Str(k), Value::Str(v))
            .unwrap();
        let inner = heap.new_table();
        unsafe { t.as_mut() }
            .set(&mut heap, Value::Int(1), Value::Table(inner))
            .unwrap();
        unsafe { inner.as_mut() }.set_metatable(Some(t));
        assert_eq!(heap.live_objects(), 4);
        // root only the outer table: everything reachable through it survives
        assert_eq!(heap.collect(&[Value::Table(t)]), 0);
        assert_eq!(heap.live_objects(), 4);
        assert_eq!(heap.collect(&[]), 4);
    }

    #[test]
    fn interned_string_reclaimed_and_reinternable() {
        let mut heap = Heap::new();
        heap.intern(b"transient");
        assert_eq!(heap.collect(&[]), 1);
        let s2 = heap.intern(b"transient");
        assert_eq!(s2.as_bytes(), b"transient");
        assert_eq!(heap.live_objects(), 1);
    }

    #[test]
    fn bytes_and_live_round_trip_to_zero() {
        // Memory-invariant audit: after a churn of table allocation, rehash-
        // driven growth, and full collection of an empty root set, both
        // `heap.bytes` and `heap.live_objects` must return to 0. Catches any
        // alloc / free asymmetry in the Table internal-Box delta tracking
        // (4bab3c5) or the live counter (link/sweep symmetry).
        let mut heap = Heap::new();
        assert_eq!(heap.bytes(), 0);
        assert_eq!(heap.live_objects(), 0);
        // Build a churn: 50 tables, each filled with 200 int keys (forces
        // multiple rehashes); plus interned strings spliced through the
        // hash part. Bytes should grow well past the empty baseline.
        let mut roots: Vec<Value> = Vec::new();
        for ti in 0..50 {
            let t = heap.new_table();
            for k in 1..=200 {
                let _ =
                    unsafe { t.as_mut() }.set(&mut heap, Value::Int(k), Value::Int(ti * 1000 + k));
            }
            for sk in 0..32 {
                let key = Value::Str(heap.intern(format!("k{ti}-{sk}").as_bytes()));
                let _ = unsafe { t.as_mut() }.set(&mut heap, key, Value::Int(sk));
            }
            roots.push(Value::Table(t));
        }
        let live_peak = heap.live_objects();
        let bytes_peak = heap.bytes();
        assert!(live_peak > 0, "live should be >0 after churn");
        assert!(bytes_peak > 0, "bytes should be >0 after churn");
        // Root only half — the other half should be collected.
        let half = roots.len() / 2;
        let freed = heap.collect(&roots[..half]);
        assert!(freed > 0, "some objects should have been freed");
        assert!(
            heap.bytes() < bytes_peak,
            "bytes must drop after partial collect"
        );
        assert!(
            heap.live_objects() < live_peak,
            "live must drop after partial collect"
        );
        // Drop everything: counters must return to 0 exactly.
        drop(roots);
        let _ = heap.collect(&[]);
        assert_eq!(heap.live_objects(), 0, "live not zero after full collect");
        assert_eq!(
            heap.bytes(),
            0,
            "bytes not zero after full collect — asymmetric alloc/free"
        );
    }

    /// Regression for `.dev/known-bugs/stringtable-intern-uaf.md`:
    /// `StringTable::intern` must NOT return a dead-white (about-to-be-swept)
    /// short-string pointer. Mirrors PUC `luaS_new`'s resurrect-on-hit guard
    /// (lstring.c — `if (isdead(g, ts)) changewhite(ts);`).
    ///
    /// Without the guard, the bucket-chain still references the unswept
    /// short string after the atomic flip; a re-`intern` of the same bytes
    /// hands back that pointer; the budget-paced sweep then frees it and
    /// the next bucket walk dereferences libc-recycled garbage (the
    /// `0x800002a80000002d` misaligned pointer seen in the audit).
    #[test]
    fn intern_resurrects_dead_white_short_string() {
        let mut heap = Heap::new();
        let alive = heap.intern(b"keep-me-alive-1");
        let dying = heap.intern(b"transient-x");
        let dying_ptr = dying.as_ptr();
        let dying_bytes = dying.as_bytes().to_vec();
        // Drive an incremental cycle by hand to reproduce the race:
        //   1. mark-propagate with `alive` only as a root → `dying` stays white
        //   2. atomic flip → `dying` becomes dead-white, bucket still points at it
        //   3. RE-INTERN the same bytes BEFORE sweep clears the bucket
        //   4. fix must either (a) skip the dead entry & alloc fresh, or
        //      (b) resurrect dying back to current-white
        let alive_root = [Value::Str(alive)];
        heap.gc_start_propagate(&alive_root, &[]);
        while !heap.gc_step_propagate(usize::MAX) {}
        heap.gc_finish_atomic();
        // At this point sweep_cur holds the detached old-heap list and the
        // dying string is dead-white. The bucket chain in `self.strings`
        // still references it.
        let resurrected = heap.intern(&dying_bytes);
        // Two valid outcomes:
        //   * resurrect: same pointer, but flagged current-white so sweep
        //     will keep it alive (PUC luaS_new shape)
        //   * skip-and-alloc-fresh: different pointer, dying gets swept
        //     normally as it should
        // Either way, after completing the sweep the heap must NOT crash
        // when we try to intern more short strings (which walks bucket chains).
        while !heap.gc_sweep_step(usize::MAX) {}
        // Smoke: bucket-chain walk for a fresh string must not deref a
        // freed pointer.
        let _fresh = heap.intern(b"after-sweep-canary");
        // If the fix is "resurrect", same pointer + bytes preserved:
        if resurrected.as_ptr() == dying_ptr {
            assert_eq!(resurrected.as_bytes(), dying_bytes.as_slice());
        }
        // Final cleanup: full collect must complete without UAF.
        drop(heap);
    }

    #[test]
    fn deep_table_chain_marks_iteratively() {
        // deep chain: explicit mark stack must not overflow (smaller under
        // miri — the interpreter makes 100k tables take ~30 minutes)
        let n = if cfg!(miri) { 2_000 } else { 100_000 };
        let mut heap = Heap::new();
        let head = heap.new_table();
        let mut cur = head;
        for _ in 0..n {
            let next = heap.new_table();
            unsafe { cur.as_mut() }
                .set(&mut heap, Value::Int(1), Value::Table(next))
                .unwrap();
            cur = next;
        }
        assert_eq!(heap.collect(&[Value::Table(head)]), 0);
        assert_eq!(heap.collect(&[]), n + 1);
    }
}
