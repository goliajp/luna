//! What PUC allocates making a state, which luna counts from PUC's own
//! record; strings luna makes for itself.

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

    /// A long string of PUC's of `size` bytes (counted now with `count`)
    /// lives as long as the object behind `h`, which holds luna's string
    /// in its place (the package table: luna's default path is a short
    /// string all package tables share).
    pub(crate) fn acct_stand_on(&mut self, h: *mut GcHeader, size: usize, count: bool) {
        if count {
            self.acct_obj(PucKind::LongStr, size);
        }
        self.stand_sizes_on(h, vec![size]);
    }

    /// Objects of PUC's of `sizes` bytes, counted already, live as long
    /// as the object behind `h`.
    pub(crate) fn stand_sizes_on(&mut self, h: *mut GcHeader, sizes: Vec<usize>) {
        self.puc.stand_ins.push((h, sizes));
        // SAFETY: `h` heads a live object; only its class byte is written
        unsafe { (*h).cls |= super::STANDS };
    }

    /// Whether objects of PUC's stand on the object behind `h`.
    pub(crate) fn stands(&self, h: *mut GcHeader) -> bool {
        // SAFETY: `h` heads a live object; its class byte is read
        unsafe { (*h).cls & super::STANDS != 0 }
    }

    /// PUC's bytes of an array part of `n` slots.
    pub(crate) fn array_bytes_of(&self, n: usize) -> usize {
        self.array_bytes(n)
    }

    /// PUC's bytes of a hash part of `n` nodes.
    pub(crate) fn hash_bytes_of(&self, n: usize) -> usize {
        self.hash_bytes(n)
    }

    /// PUC's default `package.path`, from its record of `luaL_openlibs`.
    pub(crate) fn puc_default_path(&self) -> Option<&'static [u8]> {
        let (_, strs) = crate::runtime::puc_libs::record(self.table_dialect, false);
        strs.iter()
            .copied()
            .find(|b| b.windows(5).any(|w| w == b"?.lua"))
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
}

impl Heap {
    /// A string just made: counted, or, while luna builds its own objects,
    /// marked as one PUC has not made.
    #[inline]
    pub(crate) fn acct_new_string(&mut self, h: *mut GcHeader, len: usize) {
        // 5.1's string table holds every string, 5.2's the short ones
        let held = match self.table_dialect {
            Dialect::L51 => true,
            Dialect::L52 => len <= self.puc.short_max,
            _ => false,
        };
        // SAFETY: `h` heads the string just made; only its class byte is written
        unsafe { (*h).cls = u8::from(held) * super::UNLISTED };
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
}
