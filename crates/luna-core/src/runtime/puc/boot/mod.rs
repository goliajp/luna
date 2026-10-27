//! What PUC allocates making a state, which luna counts from PUC's own
//! record; strings luna makes for itself.

use super::*;

pub(crate) mod counts;
use crate::runtime::dialect::Dialect;

impl Heap {
    /// What PUC's `lua_newstate` allocates, in its order, once the dialect
    /// is known; from here on luna's allocations count. The tag-method
    /// names and reserved words are made here, as PUC makes them; the
    /// memory error message, which the heap made before it knew the
    /// dialect, is counted where PUC makes it.
    pub(in crate::runtime::puc) fn acct_boot(&mut self) -> Result<(), crate::runtime::mem::Oom> {
        use crate::runtime::puc::boot::counts::{boot_sizes, reserved_words, tm_names};
        let d = self.puc.dialect;
        let b = boot_sizes(d);
        let s = self.puc.sizes;
        self.puc.count.silent = 0;
        self.acct_obj(PucKind::Thread, b.lg);
        // `lua_newstate`: no debt (5.1 sets its threshold when the state is
        // built)
        self.puc.pace.set_debt(d, self.puc.count.bytes, 0);
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
            self.acct_table_resize((0, 0), (b.registry_array, 0));
            self.acct_obj(PucKind::Table, s.table);
        }
        self.acct_block(0, b.strtab * s.ptr);
        self.puc.records.strt_size = b.strtab;
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
            self.strt_settle();
        }
        if d == Dialect::L51 {
            self.puc.pace.set_threshold(4 * self.puc.count.bytes as i64);
        }
        Ok(())
    }
}

impl Heap {
    /// `cs`, a long string luna's compiler made without counting, is the
    /// one PUC's scanner made as `ss` (counted) and keeps as the constant:
    /// the count moves to `cs`.
    pub(in crate::runtime) fn acct_move_long(&mut self, ss: Gc<LuaStr>, cs: Gc<LuaStr>) {
        if !ss.hdr().is_uncounted() && cs.hdr().is_uncounted() {
            // SAFETY: both are handles, so their objects are live; nothing
            // borrows the headers while their per-type bytes are written
            unsafe {
                (*ss.header()).set_uncounted(true);
                (*cs.header()).set_uncounted(false);
            }
        }
    }
}

impl Heap {
    /// A string just made: counted, or, while luna builds its own objects,
    /// marked as one PUC has not made.
    #[inline]
    pub(in crate::runtime::puc) fn acct_new_string(&mut self, s: Gc<LuaStr>, len: usize) {
        let h = s.header();
        // 5.1's string table holds every string, 5.2's the short ones
        let held = match self.puc.dialect {
            Dialect::L51 => true,
            Dialect::L52 => len <= self.puc.records.short_max,
            _ => false,
        };
        // SAFETY: `h` comes from a handle, so its string is live; nothing
        // borrows the header while its class and per-type bytes are written
        unsafe { (*h).cls = u8::from(held) * super::UNLISTED };
        if !self.counting() {
            // SAFETY: as above
            unsafe { (*h).set_uncounted(true) };
        } else {
            self.acct_string(len);
            if held {
                self.strt_enter(s);
            }
        }
    }

    /// A string luna made for itself is made by the program now: PUC makes
    /// it here.
    #[cold]
    #[inline(never)]
    pub(in crate::runtime) fn acct_promote(&mut self, s: Gc<LuaStr>, len: usize) {
        let h = s.header();
        if !self.counting() {
            return;
        }
        // SAFETY: `h` comes from a handle, so its string is live; nothing
        // borrows the header while its per-type byte is written
        unsafe {
            self.born(h);
            (*h).set_uncounted(false);
        }
        self.acct_string(len);
        self.strt_enter(s);
    }

    /// The parser's string of the chunk name `s`: counted now if luna made
    /// it without counting (PUC found it made already otherwise).
    pub(in crate::runtime) fn acct_source(&mut self, s: Gc<LuaStr>) {
        if s.hdr.sub & UNCOUNTED != 0 {
            self.acct_promote(s, s.len());
        }
    }
}
