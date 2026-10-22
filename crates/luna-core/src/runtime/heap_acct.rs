//! PUC's `totalbytes` and the events that change it.
//!
//! Every object and every block PUC would allocate is counted at PUC's
//! size for it, in the order PUC allocates it, so `collectgarbage("count")`
//! and the collector's pacing see what they see in PUC. luna's own objects
//! are laid out differently (and count separately, in the allocation
//! context); nothing here touches them.

use super::*;
use crate::runtime::puc_size::PucSizes;
use crate::runtime::table::Dialect;

#[path = "heap_ghost.rs"]
mod ghost;
#[path = "heap_keeper.rs"]
pub(in crate::runtime::heap) mod keeper;
#[path = "heap_strcache.rs"]
pub(crate) mod strcache;
pub(crate) use ghost::{Ghost, GhostRef};
#[path = "heap_parse.rs"]
pub(crate) mod parse;
#[path = "heap_acct_state.rs"]
mod state;
pub(crate) use state::PucState;

/// What a counted allocation is, for PUC's tag of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PucKind {
    ShortStr,
    LongStr,
    Table,
    LuaClosure,
    CClosure,
    Userdata,
    Thread,
    Proto,
    Upval,
}

impl PucKind {
    /// The tag PUC passes its allocation function for a new object (5.1
    /// passes none).
    #[cfg(feature = "gc-trace")]
    fn tag(self, d: Dialect) -> usize {
        use PucKind::*;
        match (d, self) {
            (Dialect::L51, _) => 0,
            (_, ShortStr) => 4,
            // PUC passes the base type, not the variant
            (_, LongStr) => 4,
            (_, Table) => 5,
            (_, LuaClosure) => 6,
            (_, CClosure) => 6,
            (_, Userdata) => 7,
            (_, Thread) => 8,
            // 5.3's upvalues are plain blocks
            (Dialect::L53, Upval) => 0,
            (Dialect::L52 | Dialect::L53, Proto) => 9,
            (Dialect::L52, Upval) => 10,
            (_, Upval) => 9,
            (_, Proto) => 10,
        }
    }
}

impl Heap {
    /// Take on `version`'s rules, then count what PUC's `lua_newstate`
    /// allocates ([`Heap::acct_boot`]).
    pub(crate) fn set_version(
        &mut self,
        version: crate::version::LuaVersion,
    ) -> Result<(), crate::runtime::mem::Oom> {
        use crate::version::LuaVersion;
        // PUC 5.1 had no ephemeron pass: `__mode='k'` tables marked their
        // values strongly (gc.lua's "weak tables" section relies on that)
        self.no_ephemeron = version <= LuaVersion::Lua51;
        self.refinalize = version != LuaVersion::Lua52;
        self.signed_zero_keys = version <= LuaVersion::Lua52;
        let d = Dialect::of(version);
        self.table_dialect = d;
        self.sizes = PucSizes::of(d);
        // the strings the heap made before it knew the dialect (its fixed
        // memory error message) move to the dialect's hash
        self.hash51 = version == LuaVersion::Lua51;
        if self.hash51 {
            self.puc.short_max = usize::MAX;
        }
        let seed = self.seed();
        self.rehash_strings(seed);
        // PUC 5.3 needs two GC cycles to finalize a table caught in a
        // coroutine reference cycle (gc.lua :502); 5.4+ finalize it in one
        self.defer_thread_cycle_finalize = version == LuaVersion::Lua53;
        self.acct_boot()
    }

    /// A new object of `size` bytes.
    #[inline]
    pub(crate) fn acct_obj(&mut self, kind: PucKind, size: usize) {
        if self.puc.silent > 0 {
            return;
        }
        self.bytes += size;
        #[cfg(feature = "gc-trace")]
        {
            let tag = kind.tag(self.table_dialect);
            self.trace.line(format_args!("a {tag} {size}"));
        }
        #[cfg(not(feature = "gc-trace"))]
        let _ = kind;
    }

    /// A new object (not a table, not a string) `h` of `size` bytes:
    /// counted, or, while luna builds objects of its own, marked as one PUC
    /// has not made, whose free is not counted either (see
    /// [`Heap::acct_promote_obj`]).
    pub(crate) fn acct_new(&mut self, h: *mut GcHeader, kind: PucKind, size: usize) {
        if self.puc.silent > 0 {
            set_uncounted(h, true);
        } else {
            self.acct_obj(kind, size);
        }
    }

    /// An object made without counting is PUC's now: counted here.
    pub(crate) fn acct_promote_obj(&mut self, h: *mut GcHeader, kind: PucKind, size: usize) {
        if self.puc.silent == 0 && is_uncounted(h) {
            // SAFETY: the caller names a live object
            unsafe { self.born(h) };
            set_uncounted(h, false);
            self.acct_obj(kind, size);
        }
    }

    /// A block (an array, a buffer) going from `old` to `new` bytes; either may be 0.
    #[inline]
    pub(crate) fn acct_block(&mut self, old: usize, new: usize) {
        if self.puc.silent > 0 {
            return;
        }
        self.bytes = self.bytes + new - old;
        #[cfg(feature = "gc-trace")]
        match (old, new) {
            (0, 0) => {}
            (0, n) => self.trace.line(format_args!("a 0 {n}")),
            (o, 0) => self.trace.line(format_args!("d {o}")),
            (o, n) => self.trace.line(format_args!("r {o} {n}")),
        }
    }

    /// A block PUC allocates through the allocation function but leaves out of
    /// `totalbytes` (a `luaL_Buffer` box's contents): only the trace sees it.
    #[inline]
    pub(crate) fn trace_raw(&mut self, old: usize, new: usize) {
        #[cfg(feature = "gc-trace")]
        if self.puc.silent == 0 {
            self.trace.raw = true;
            match (old, new) {
                (0, 0) => {}
                (0, n) => self.trace.line(format_args!("a 0 {n}")),
                (o, 0) => self.trace.line(format_args!("d {o}")),
                (o, n) => self.trace.line(format_args!("r {o} {n}")),
            }
            self.trace.raw = false;
        }
        #[cfg(not(feature = "gc-trace"))]
        let _ = (old, new);
    }

    /// An object of `size` bytes freed.
    #[inline]
    pub(crate) fn acct_free(&mut self, size: usize) {
        if self.puc.silent > 0 {
            return;
        }
        self.bytes -= size;
        #[cfg(feature = "gc-trace")]
        self.trace.line(format_args!("d {size}"));
    }
}

#[path = "heap_acct_boot.rs"]
mod boot;
#[path = "heap_acct_defer.rs"]
mod defer;
#[path = "heap_acct_fmt.rs"]
mod fmt;
#[path = "heap_acct_parts.rs"]
mod parts;
#[path = "heap_acct_thread.rs"]
mod thread;
pub(crate) use boot::STRT_STAND;
pub(crate) use defer::Deferred;
pub(crate) use parts::ENV51;
pub(crate) use thread::BASIC_CI_51;

/// Whether object `h` is one PUC has not made (see [`Heap::acct_new`]).
#[inline(always)]
pub(crate) fn is_uncounted(h: *const GcHeader) -> bool {
    // SAFETY: `h` heads a live object of this heap; its header is read
    unsafe { (*h).sub & UNCOUNTED != 0 }
}

/// in a string's `sub` byte: a 5.5 external string over bytes PUC does not
/// own (`LSTRFIX`), its header alone counted
pub(crate) const FIXED_EXT: u8 = 0x20;

pub(super) fn set_uncounted(h: *mut GcHeader, on: bool) {
    // SAFETY: `h` heads a live object of this heap; only its per-type byte
    // is written, which no type but a table uses otherwise
    unsafe {
        if on {
            (*h).sub |= UNCOUNTED;
        } else {
            (*h).sub &= !UNCOUNTED;
        }
    }
}
