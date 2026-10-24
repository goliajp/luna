//! PUC's sizes of tables and strings, and the order PUC
//! allocates their parts in.

use super::*;
use crate::runtime::dialect::Dialect;

impl Heap {
    /// The bytes of a table's array part of `n` slots.
    pub(in crate::runtime) fn array_bytes(&self, n: usize) -> usize {
        crate::runtime::puc::work::array_bytes(&self.puc.sizes, self.puc.dialect, n)
    }

    /// The bytes of a table's hash part of `h` nodes.
    pub(in crate::runtime) fn hash_bytes(&self, h: usize) -> usize {
        crate::runtime::puc::work::hash_bytes(&self.puc.sizes, self.puc.dialect, h)
    }

    /// A table's parts going from `old_a` array slots and `old_h` nodes to
    /// `new_a` and `new_h`, as PUC's `luaH_resize` allocates and frees them:
    /// up to 5.3 a growing array first, then the new nodes, a shrinking
    /// array, the old nodes; from 5.4 the new nodes, the array, the old
    /// nodes. A part of no nodes is PUC's shared dummy node, never
    /// allocated. For a table of PUC's luna has none for (a parser's,
    /// the registry PUC's state starts with): counted unless luna is
    /// building its own objects.
    pub(crate) fn acct_table_resize(&mut self, old: (usize, usize), new: (usize, usize)) {
        if self.counting() {
            self.table_parts(old, new, Heap::acct_block);
        }
    }

    /// [`Heap::acct_table_resize`] of a counted table of luna's: counted
    /// as the table is, whatever luna builds meanwhile (see
    /// [`Heap::acct_part`]).
    pub(crate) fn acct_counted_table_resize(&mut self, old: (usize, usize), new: (usize, usize)) {
        self.table_parts(old, new, Heap::acct_part);
    }

    fn table_parts(
        &mut self,
        (old_a, old_h): (usize, usize),
        (new_a, new_h): (usize, usize),
        part: fn(&mut Heap, usize, usize),
    ) {
        let (oh, nh) = (self.hash_bytes(old_h), self.hash_bytes(new_h));
        let (oa, na) = (self.array_bytes(old_a), self.array_bytes(new_a));
        // with no trace to write, only the sum is seen
        if !crate::runtime::gc_hooks::TRACE {
            return part(self, oh + oa, nh + na);
        }
        match self.puc.dialect {
            Dialect::L51 | Dialect::L52 | Dialect::L53 => {
                if new_a > old_a {
                    part(self, oa, na);
                }
                part(self, 0, nh);
                if new_a < old_a {
                    part(self, oa, na);
                }
                part(self, oh, 0);
            }
            Dialect::L54 => {
                // the array is reallocated even at the same size
                part(self, 0, nh);
                part(self, oa, na);
                part(self, oh, 0);
            }
            Dialect::L55 => {
                part(self, 0, nh);
                if new_a != old_a {
                    part(self, 0, na);
                    part(self, oa, 0);
                }
                part(self, oh, 0);
            }
        }
    }

    /// A new table `h`: counted, or, made while nothing is counted, luna's
    /// own, a table PUC does not have, which nothing counts as long as it
    /// lives (its parts as they grow, its free).
    #[inline]
    pub(crate) fn acct_new_table(&mut self, h: Gc<GcHeader>) {
        if !self.counting() {
            // SAFETY: `h` is a handle, so its table is live; nothing borrows
            // the header while its class byte is written
            unsafe { (*h.as_ptr()).cls |= super::UNLISTED };
        } else {
            self.acct_obj(PucKind::Table, self.puc.sizes.table);
        }
    }

    /// `t`, made while nothing was counted, is a table PUC's state starts
    /// with, which [`Heap::acct_boot`] counted in its place: counted from
    /// here on, as it grows and when it is freed.
    pub(crate) fn boot_table(t: Gc<crate::runtime::Table>) -> Gc<crate::runtime::Table> {
        // SAFETY: `t` is a live table; only its class byte is written
        unsafe { (*t.header()).cls &= !super::UNLISTED };
        t
    }

    /// Whether `t` is one of luna's own tables, which nothing counts.
    #[inline]
    pub(crate) fn is_own_table(&self, t: &crate::runtime::Table) -> bool {
        t.hdr.cls & super::UNLISTED != 0
    }

    /// A counted table freed with `a` array slots and `h` nodes (PUC
    /// `luaH_free`).
    pub(crate) fn acct_table_free(&mut self, a: usize, h: usize) {
        let (ab, hb) = (self.array_bytes(a), self.hash_bytes(h));
        self.acct_part(hb, 0);
        self.acct_part(ab, 0);
        let t = self.puc.sizes.table;
        self.acct_part(t, 0);
    }
}

impl Heap {
    /// A new string of `len` bytes (PUC `createstrobj`).
    #[inline]
    pub(crate) fn acct_string(&mut self, len: usize) {
        // a short string with no trace to write: only the sum is seen
        // (the table grows on the same condition either side of the
        // string's own count)
        if !crate::runtime::gc_hooks::TRACE && self.counting() {
            if len > self.puc.short_max {
                let l55 = self.puc.dialect == Dialect::L55;
                self.puc.bytes += self.puc.sizes.string(len, l55);
                return;
            }
            if self.puc.strt_nuse >= self.puc.strt_size {
                self.grow_strtab();
            }
            self.puc.bytes += self.puc.sizes.string(len, false);
            self.puc.strt_nuse += 1;
            return;
        }
        let d = self.puc.dialect;
        let long = len > crate::runtime::string::MAX_SHORT_LEN && d != Dialect::L51;
        let kind = if long {
            PucKind::LongStr
        } else {
            PucKind::ShortStr
        };
        let size = self.puc.sizes.string(len, long && d == Dialect::L55);
        if long || !self.counting() {
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
    /// old, later versions reallocate. The table is PUC's state's: its
    /// blocks count whatever luna builds meanwhile (see
    /// [`Heap::acct_part`]), as the strings it holds come and go.
    #[cold]
    fn grow_strtab(&mut self) {
        let (old, new) = (self.puc.strt_size, self.puc.strt_size * 2);
        let p = self.puc.sizes.ptr;
        if self.puc.dialect == Dialect::L51 {
            self.acct_part(0, new * p);
            self.acct_part(old * p, 0);
        } else {
            self.acct_part(old * p, new * p);
        }
        self.puc.strt_size = new;
        if matches!(self.puc.dialect, Dialect::L51 | Dialect::L52) {
            self.strt_grew(old);
        }
    }

    /// PUC's `checkSizes` when a collection's sweep ends: the string table
    /// halved when a quarter of it is used (5.1: and it is past twice its
    /// minimum; 5.2: when less than half is), and 5.1's concatenation
    /// buffer halved past twice its minimum, 5.2's freed.
    pub(crate) fn acct_check_sizes(&mut self) {
        let d = self.puc.dialect;
        let (size, nuse) = (self.puc.strt_size, self.puc.strt_nuse);
        let shrink = match d {
            Dialect::L51 => nuse < size / 4 && size > 64,
            Dialect::L52 => nuse < size / 2,
            _ => nuse < size / 4,
        };
        if shrink {
            let (old, new) = (size, size / 2);
            let p = self.puc.sizes.ptr;
            if d == Dialect::L51 {
                self.acct_part(0, new * p);
                self.acct_part(old * p, 0);
            } else {
                self.acct_part(old * p, new * p);
            }
            self.puc.strt_size = new;
            self.strt_resized(old);
            // 5.3 and 5.4 correct the estimate by what the shrink freed
            if matches!(d, Dialect::L53 | Dialect::L54) {
                self.puc.pace.estimate -= ((old - new) * p) as i64;
            }
        }
        match d {
            Dialect::L51 if self.puc.gbuff.size() > 64 => {
                self.shrink_gbuff(self.puc.gbuff.size() / 2)
            }
            Dialect::L52 => self.shrink_gbuff(0),
            _ => {}
        }
    }
}

impl Heap {
    /// A counted string of `len` bytes freed: taken off the count (and
    /// PUC's string table) whatever luna builds meanwhile, as it was
    /// counted (see [`Heap::acct_part`]).
    #[inline]
    pub(crate) fn acct_string_free(&mut self, len: usize) {
        if self.is_dropping() {
            return;
        }
        let long = len > self.puc.short_max;
        if !long {
            self.puc.strt_nuse -= 1;
        }
        let size = self
            .puc
            .sizes
            .string(len, long && self.puc.dialect == Dialect::L55);
        self.count_part(size, 0);
    }
}
