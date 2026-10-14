//! What PUC allocates making a state and opening its libraries, which luna
//! counts from PUC's own record; strings luna makes for itself; CallInfos.

use super::*;
use crate::runtime::table::Dialect;

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

    /// The parser's string of the chunk name `s`: counted now if luna made
    /// it without counting (PUC found it made already otherwise).
    pub(crate) fn acct_source(&mut self, s: Gc<LuaStr>) {
        if s.hdr.sub & UNCOUNTED != 0 {
            self.acct_promote(s.as_ptr() as *mut GcHeader, s.len());
        }
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
