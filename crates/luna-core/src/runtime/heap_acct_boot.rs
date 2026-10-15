//! What PUC allocates making a state and opening its libraries, which luna
//! counts from PUC's own record; strings luna makes for itself.

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
        // `lua_newstate`: no debt (5.1 sets its threshold when the state is
        // built)
        self.pace.set_debt(d, self.bytes, 0);
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
        // PUC fixes these (`luaC_fix`, 5.1 `luaS_fix`): never collected,
        // nor marked
        for name in tm_names(d) {
            let s = self.try_intern(name.as_bytes())?;
            self.fix_string(s);
        }
        if d >= Dialect::L53 {
            let s = self.try_intern(b"_ENV")?;
            self.fix_string(s);
        }
        for w in reserved_words(d) {
            let s = self.try_intern(w.as_bytes())?;
            self.fix_string(s);
        }
        if d <= Dialect::L52 {
            self.acct_string(memerr);
        }
        if d == Dialect::L51 {
            self.pace.set_threshold(4 * self.bytes as i64);
        }
        Ok(())
    }
}

impl Heap {
    /// `s`, a long string luna's libraries made without counting, is the
    /// one PUC's record made and kept with the same bytes: counted there,
    /// it is PUC's now.
    pub(crate) fn acct_adopt_long(&mut self, s: Gc<LuaStr>) {
        let h = s.as_ptr() as *mut GcHeader;
        let kept = self.puc.kept_long.iter().find(|k| k.0 == s.as_bytes());
        if let Some(&(_, size)) = kept
            && is_uncounted(h)
        {
            super::set_uncounted(h, false);
            // SAFETY: `s` is a live string; only its per-type byte changes
            unsafe { (*h).sub |= self.external_kind(size) };
        }
    }

    /// The flag of a 5.5 long string PUC made at `size` bytes: one over
    /// bytes it owns (`LSTRMEM`) or over bytes it does not (`LSTRFIX`),
    /// which its size alone tells from a regular one.
    fn external_kind(&self, size: usize) -> u8 {
        let hdr = self.sizes.lngstr_hdr;
        match self.table_dialect {
            Dialect::L55 if size == hdr + 2 * self.sizes.ptr => super::EXTERNAL,
            Dialect::L55 if size == hdr => super::FIXED_EXT,
            _ => 0,
        }
    }

    /// `cs`, a long string luna's compiler made without counting, is the
    /// one PUC's scanner made as `ss` (counted) and keeps as the constant:
    /// the count moves to `cs`.
    pub(crate) fn acct_move_long(&mut self, ss: Gc<LuaStr>, cs: Gc<LuaStr>) {
        let (from, to) = (ss.as_ptr() as *mut GcHeader, cs.as_ptr() as *mut GcHeader);
        if !is_uncounted(from) && is_uncounted(to) {
            super::set_uncounted(from, true);
            super::set_uncounted(to, false);
        }
    }

    /// `s`, a string luna's libraries made without counting, stands where
    /// PUC keeps the long string of `bytes` from its record (luna's own
    /// default `package.path`): when `s` is freed, PUC's is.
    pub(crate) fn acct_stand_in(&mut self, s: Gc<LuaStr>, puc: (&[u8], usize)) {
        let h = s.as_ptr() as *mut GcHeader;
        if is_uncounted(h) {
            self.puc.stand_ins.push((h, vec![puc.1]));
        }
    }
}

impl Heap {
    /// A string just made: counted, or, while luna builds its own objects,
    /// marked as one PUC has not made.
    #[inline]
    pub(crate) fn acct_new_string(&mut self, h: *mut GcHeader, len: usize) {
        if self.puc.silent > 0 {
            super::set_uncounted(h, true);
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
        super::set_uncounted(h, false);
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
        let mut live = true;
        let mut size = 0;
        let ops = ops.as_chunks::<3>().0;
        for (i, op) in ops.iter().enumerate() {
            let (x, y) = (op[1] as usize, op[2] as usize);
            match op[0] {
                3 => self.replay_string(strs[x], live, size)?,
                k => {
                    live = k != 4;
                    size = y;
                    self.raw_event(k, x, y);
                    // garbage that is not a string: 5.1's C closures of the
                    // `luaopen_` functions `luaL_openlibs` calls
                    if k == 4 && ops.get(i + 1).is_none_or(|n| n[0] != 3) {
                        self.ghost_record(PucKind::CClosure, y)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The string of `bytes` PUC's record made, at `size` bytes.
    fn replay_string(
        &mut self,
        bytes: &'static [u8],
        live: bool,
        size: usize,
    ) -> Result<(), crate::runtime::mem::Oom> {
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
        if bytes.len() > crate::runtime::string::MAX_SHORT_LEN && self.table_dialect != Dialect::L51
        {
            // luna does not intern long strings (but 5.1's): nothing to
            // find later; one luna's libraries keep too is theirs
            // (`acct_adopt_long`)
            if live {
                self.puc.kept_long.push((bytes, size));
            }
            return Ok(());
        }
        if !live
            && let Some(s) = self.find_interned(bytes)
            && is_uncounted(s.as_ptr() as *mut GcHeader)
        {
            // a string luna keeps for itself and PUC drops: PUC's (counted
            // with the op before) is garbage, luna's stays out of the
            // count until the program makes one
            let size = self.sizes.string(bytes.len(), false);
            return self.ghost_record(PucKind::ShortStr, size);
        }
        self.puc.silent += 1;
        let s = self.try_intern(bytes);
        self.puc.silent -= 1;
        let s = s?;
        super::set_uncounted(s.as_ptr() as *mut GcHeader, false);
        if live {
            self.puc.pins.push(s);
        }
        Ok(())
    }
}
