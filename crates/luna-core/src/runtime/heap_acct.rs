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

/// PUC's structures luna has no object for, as far as the count goes.
pub(crate) struct PucState {
    /// above 0, nothing is counted: objects PUC made at another point (or
    /// not at all) are being built
    pub(crate) silent: u32,
    /// the string table's size and its number of strings
    pub(crate) strt_size: usize,
    pub(crate) strt_nuse: usize,
    /// strings PUC's libraries keep and luna's may not: kept alive
    pub(crate) pins: Vec<Gc<crate::runtime::LuaStr>>,
    /// 5.2+: the CallInfos the main thread has allocated, and how deep its
    /// calls are now
    pub(crate) ci_count: usize,
    pub(crate) depth: usize,
}

impl Default for PucState {
    fn default() -> PucState {
        // nothing is counted until the dialect is known (`acct_boot`)
        PucState {
            silent: 1,
            strt_size: 0,
            strt_nuse: 0,
            pins: Vec::new(),
            ci_count: 0,
            depth: 0,
        }
    }
}

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

    /// A block (an array, a buffer) going from `old` to `new` bytes; either
    /// may be 0.
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
#[path = "heap_acct_parts.rs"]
mod parts;
