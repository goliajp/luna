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

impl Heap {
    /// The bytes of a table's array part of `n` slots.
    fn array_bytes(&self, n: usize) -> usize {
        match (self.table_dialect, n) {
            (_, 0) => 0,
            // 5.5 keeps the values, then a byte per tag, around an unsigned
            (Dialect::L55, n) => n * 9 + 4,
            (_, n) => n * self.sizes.tvalue,
        }
    }

    /// A table's parts going from `old_a` array slots and `old_h` nodes to
    /// `new_a` and `new_h`, as PUC's `luaH_resize` allocates and frees them:
    /// up to 5.3 a growing array first, then the new nodes, a shrinking
    /// array, the old nodes; from 5.4 the new nodes, the array, the old
    /// nodes. A part of no nodes is PUC's shared dummy node, never
    /// allocated.
    pub(crate) fn acct_table_resize(
        &mut self,
        old_a: usize,
        old_h: usize,
        new_a: usize,
        new_h: usize,
    ) {
        let node = self.sizes.node;
        let (oa, na) = (self.array_bytes(old_a), self.array_bytes(new_a));
        match self.table_dialect {
            Dialect::L51 | Dialect::L52 | Dialect::L53 => {
                if new_a > old_a {
                    self.acct_block(oa, na);
                }
                self.acct_block(0, new_h * node);
                if new_a < old_a {
                    self.acct_block(oa, na);
                }
                self.acct_block(old_h * node, 0);
            }
            Dialect::L54 => {
                // the array is reallocated even at the same size
                self.acct_block(0, new_h * node);
                self.acct_block(oa, na);
                self.acct_block(old_h * node, 0);
            }
            Dialect::L55 => {
                self.acct_block(0, new_h * node);
                if new_a != old_a {
                    self.acct_block(0, na);
                    self.acct_block(oa, 0);
                }
                self.acct_block(old_h * node, 0);
            }
        }
    }

    /// A table freed with `a` array slots and `h` nodes (PUC `luaH_free`).
    pub(crate) fn acct_table_free(&mut self, a: usize, h: usize) {
        let node = self.sizes.node;
        let ab = self.array_bytes(a);
        match self.table_dialect {
            Dialect::L51 | Dialect::L52 | Dialect::L53 => {
                self.acct_block(h * node, 0);
                self.acct_block(ab, 0);
            }
            _ => {
                self.acct_block(h * node, 0);
                self.acct_block(ab, 0);
            }
        }
        let t = self.sizes.table;
        self.acct_free(t);
    }
}

impl Heap {
    /// A new string of `len` bytes (PUC `createstrobj`).
    pub(crate) fn acct_string(&mut self, len: usize) {
        let d = self.table_dialect;
        let long = len > crate::runtime::string::MAX_SHORT_LEN && d != Dialect::L51;
        let kind = if long {
            PucKind::LongStr
        } else {
            PucKind::ShortStr
        };
        let size = self.sizes.string(len, long && d == Dialect::L55);
        if long || self.puc.silent > 0 {
            return self.acct_obj(kind, size);
        }
        // the string table grows when it is full: 5.2 on before the new
        // string goes in, 5.1 once it is past full
        if d != Dialect::L51 && self.puc.strt_nuse >= self.puc.strt_size {
            self.grow_strtab();
        }
        self.acct_obj(kind, size);
        self.puc.strt_nuse += 1;
        if d == Dialect::L51 && self.puc.strt_nuse > self.puc.strt_size {
            self.grow_strtab();
        }
    }

    /// The string table doubling: 5.1 allocates the new one and frees the
    /// old, later versions reallocate.
    fn grow_strtab(&mut self) {
        let (old, new) = (self.puc.strt_size, self.puc.strt_size * 2);
        let p = self.sizes.ptr;
        if self.table_dialect == Dialect::L51 {
            self.acct_block(0, new * p);
            self.acct_block(old * p, 0);
        } else {
            self.acct_block(old * p, new * p);
        }
        self.puc.strt_size = new;
    }

    /// The bytes of a prototype's arrays, in PUC's `luaF_freeproto` order.
    fn proto_arrays(&self, p: &crate::runtime::function::Proto) -> [usize; 6] {
        let s = self.sizes;
        let n = p.code.len();
        let lineinfo = match self.table_dialect {
            Dialect::L54 | Dialect::L55 => n,
            _ => 4 * n,
        };
        [
            4 * n,
            s.ptr * p.protos.len(),
            s.tvalue * p.consts.len(),
            lineinfo,
            s.locvar * p.locvars.len(),
            s.upvaldesc * p.upvals.len(),
        ]
    }

    /// A finished prototype with its arrays (their growth while compiling
    /// is not counted yet).
    pub(crate) fn acct_proto(&mut self, p: Gc<crate::runtime::function::Proto>) {
        self.acct_obj(PucKind::Proto, self.sizes.proto);
        for b in self.proto_arrays(&p) {
            self.acct_block(0, b);
        }
    }

    /// A prototype freed.
    pub(crate) fn acct_proto_free(&mut self, p: &crate::runtime::function::Proto) {
        for b in self.proto_arrays(p) {
            self.acct_block(b, 0);
        }
        let s = self.sizes.proto;
        self.acct_free(s);
    }
}

impl Heap {
    /// A string of `len` bytes freed.
    pub(crate) fn acct_string_free(&mut self, len: usize) {
        let long =
            len > crate::runtime::string::MAX_SHORT_LEN && self.table_dialect != Dialect::L51;
        if !long && self.puc.silent == 0 {
            self.puc.strt_nuse -= 1;
        }
        let size = self
            .sizes
            .string(len, long && self.table_dialect == Dialect::L55);
        self.acct_free(size);
    }
}

impl Heap {
    /// What PUC's `lua_newstate` allocates, in its order, once the dialect
    /// is known; from here on luna's allocations count. The tag-method
    /// names and reserved words are made here, as PUC makes them; the
    /// memory error message, which the heap made before it knew the
    /// dialect, is counted where PUC makes it.
    pub(crate) fn acct_boot(&mut self) -> Result<(), crate::runtime::mem::Oom> {
        use crate::runtime::puc_boot::{boot_sizes, reserved_words, tm_names};
        let d = self.table_dialect;
        let b = boot_sizes(d);
        let s = self.sizes;
        self.puc.silent = 0;
        self.acct_obj(PucKind::Thread, b.lg);
        if b.ci_entries > 0 {
            self.acct_block(0, b.ci_entries * s.callinfo);
        }
        self.acct_block(0, b.stack_slots * s.tvalue);
        if d == Dialect::L51 {
            // `luaH_new(L, 0, 2)` for the globals, then the registry
            for _ in 0..2 {
                self.acct_obj(PucKind::Table, s.table);
                self.acct_block(0, 2 * s.node);
            }
        } else {
            self.acct_obj(PucKind::Table, s.table);
            self.acct_table_resize(0, 0, b.registry_array, 0);
            self.acct_obj(PucKind::Table, s.table);
        }
        self.acct_block(0, b.strtab * s.ptr);
        self.puc.strt_size = b.strtab;
        let memerr = b"not enough memory".len();
        if d >= Dialect::L53 {
            self.acct_string(memerr);
        }
        for name in tm_names(d) {
            self.try_intern(name.as_bytes())?;
        }
        if d >= Dialect::L53 {
            self.try_intern(b"_ENV")?;
        }
        for w in reserved_words(d) {
            self.try_intern(w.as_bytes())?;
        }
        if d <= Dialect::L52 {
            self.acct_string(memerr);
        }
        Ok(())
    }
}

impl Heap {
    /// A string just made: counted, or, while luna builds its own objects,
    /// marked as one PUC has not made.
    pub(crate) fn acct_new_string(&mut self, h: *mut GcHeader, len: usize) {
        if self.puc.silent > 0 {
            // SAFETY: `h` is the string the caller just made and linked
            unsafe { (*h).sub |= UNCOUNTED };
        } else {
            self.acct_string(len);
        }
    }

    /// A string luna made for itself is made by the program now: PUC makes
    /// it here.
    #[cold]
    #[inline(never)]
    pub(crate) fn acct_promote(&mut self, h: *mut GcHeader, len: usize) {
        if self.puc.silent > 0 {
            return;
        }
        // SAFETY: `h` is a live string the caller found
        unsafe { (*h).sub &= !UNCOUNTED };
        self.acct_string(len);
    }

    fn raw_event(&mut self, op: u32, x: usize, y: usize) {
        match op {
            1 => self.bytes = self.bytes + y - x,
            2 => self.bytes -= x,
            _ => self.bytes += y,
        }
        #[cfg(feature = "gc-trace")]
        match op {
            1 => self.trace.line(format_args!("r {x} {y}")),
            2 => self.trace.line(format_args!("d {x}")),
            _ => self.trace.line(format_args!("a {x} {y}")),
        }
    }

    /// What PUC's `luaL_openlibs` allocates, replayed from its record
    /// (`puc_libs_data.rs`), after luna opened its libraries without
    /// counting. Each string PUC made is made counted in luna too (luna
    /// may not have it), and the ones PUC keeps are kept, so a later
    /// string of the same bytes finds it as in PUC.
    pub(crate) fn acct_replay_openlibs(&mut self) -> Result<(), crate::runtime::mem::Oom> {
        let (ops, strs) = crate::runtime::puc_libs::record(self.table_dialect);
        // each library opens in a call one level down (its CallInfo, when
        // a new one, is in the record)
        self.puc.ci_count = self.puc.ci_count.max(self.puc.depth + 1);
        let mut live = true;
        for op in ops.chunks_exact(3) {
            let (x, y) = (op[1] as usize, op[2] as usize);
            match op[0] {
                3 => self.replay_string(strs[x], live)?,
                k => {
                    live = k != 4;
                    self.raw_event(k, x, y);
                }
            }
        }
        Ok(())
    }

    fn replay_string(&mut self, bytes: &[u8], live: bool) -> Result<(), crate::runtime::mem::Oom> {
        let short = bytes.len() <= crate::runtime::string::MAX_SHORT_LEN
            || self.table_dialect == Dialect::L51;
        if short {
            // the string table's growth is in the record; follow its size
            if self.table_dialect != Dialect::L51 && self.puc.strt_nuse >= self.puc.strt_size {
                self.puc.strt_size *= 2;
            }
            self.puc.strt_nuse += 1;
            if self.table_dialect == Dialect::L51 && self.puc.strt_nuse > self.puc.strt_size {
                self.puc.strt_size *= 2;
            }
        }
        if bytes.len() > crate::runtime::string::MAX_SHORT_LEN {
            // luna does not intern long strings: nothing to find later
            return Ok(());
        }
        self.puc.silent += 1;
        let s = self.try_intern(bytes);
        self.puc.silent -= 1;
        let s = s?;
        // SAFETY: `s` is the live string just found or made; only its
        // header byte is written
        unsafe { (*(s.as_ptr() as *mut GcHeader)).sub &= !UNCOUNTED };
        if live {
            self.puc.pins.push(s);
        }
        Ok(())
    }
}

impl Heap {
    /// The main thread's calls are `depth` deep: 5.2+ allocate a CallInfo
    /// for each level never reached before (PUC `luaE_extendCI`).
    pub(crate) fn acct_call_depth(&mut self, depth: usize) {
        if self.table_dialect != Dialect::L51 {
            while self.puc.ci_count < depth {
                let ci = self.sizes.callinfo;
                self.acct_block(0, ci);
                self.puc.ci_count += 1;
            }
        }
        self.puc.depth = depth;
    }
}
