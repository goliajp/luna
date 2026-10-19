//! The order of each chain of PUC's string table (5.1, 5.2), which the
//! string sweep frees a bucket's dead strings in: a new string goes to
//! the head of its chain, and a resize (`luaS_resize`) takes the old
//! buckets in order, each chain from its head, putting each string at the
//! head of its new chain. A string's place is a number in its header
//! (`LuaStr::chain`), the newest of a chain highest; a bucket that runs
//! out of numbers is numbered again from 1.
//!
//! The order is seen only in the accounting trace: the string sweep takes
//! a whole bucket in a step, so what a step frees, and the count a program
//! reads, do not depend on it. Only a `gc-trace` build keeps it.

use super::*;

impl Heap {
    /// The bucket of PUC's string table of `size` buckets string `p` is in.
    fn puc_bucket(&self, p: *const LuaStr, size: usize) -> usize {
        // SAFETY: `p` is an allocated string
        let s = unsafe { Gc::from_ptr(p as *mut LuaStr) };
        let h = match self.table_dialect {
            Dialect::L52 if s.len() >= 32 => hash_52(s.as_bytes(), self.seed),
            _ => s.stored_hash(),
        };
        h as usize % size.max(1)
    }

    /// Whether PUC's string table holds string `p`.
    fn puc_holds(&self, p: *mut LuaStr) -> bool {
        let h = p as *mut GcHeader;
        // SAFETY: `p` is an allocated string
        unsafe {
            !is_uncounted(h)
                && (*h).sub & (EXTERNAL | SWEPT) == 0
                && (self.table_dialect == Dialect::L51 || (*p).is_short())
        }
    }

    /// The table of `size` buckets numbered afresh, `skip` left out: each
    /// string in the order a resize from `from` buckets puts it in its
    /// chain (the old buckets in order, each chain from its head).
    fn strt_reorder(&mut self, from: usize, size: usize, skip: *mut LuaStr) {
        if self.strt_rehash_halves(from, size, skip) {
            return;
        }
        let from = from.max(1);
        let mut v = std::mem::take(&mut self.strt.order);
        v.clear();
        self.strings.each(|p| v.push((0, 0, p)));
        v.retain(|e| e.2 != skip && self.puc_holds(e.2));
        let counts = &mut self.strt.counts;
        counts.clear();
        counts.resize(from + 1, 0);
        for e in v.iter_mut() {
            // SAFETY: the table holds allocated strings
            let (b, k) = (self.puc_bucket(e.2, from) as u32, unsafe {
                Gc::from_ptr(e.2).chain()
            });
            *e = (b, k, e.2);
            self.strt.counts[b as usize + 1] += 1;
        }
        let counts = &mut self.strt.counts;
        for i in 1..counts.len() {
            counts[i] += counts[i - 1];
        }
        let mut sorted = std::mem::take(&mut self.strt.ordered);
        sorted.clear();
        sorted.resize(v.len(), (0, 0, std::ptr::null_mut()));
        for &e in &v {
            let at = &mut self.strt.counts[e.0 as usize];
            sorted[*at as usize] = e;
            *at += 1;
        }
        // each old chain from its head, the newest (highest number) first;
        // a chain is short
        let mut start = 0;
        while start < sorted.len() {
            let mut end = start + 1;
            while end < sorted.len() && sorted[end].0 == sorted[start].0 {
                end += 1;
            }
            sorted[start..end].sort_unstable_by_key(|e| std::cmp::Reverse(e.1));
            start = end;
        }
        let seq = &mut self.strt.seq;
        seq.clear();
        seq.resize(size, 0);
        for &(_, _, p) in &sorted {
            let b = self.puc_bucket(p, size);
            let seq = &mut self.strt.seq;
            seq[b] += 1;
            // SAFETY: as above
            unsafe { Gc::from_ptr(p).set_chain(seq[b]) };
        }
        self.strt.order = v;
        self.strt.ordered = sorted;
    }

    /// [`Heap::strt_reorder`] for a table doubled or halved (PUC's resizes
    /// always are), in one pass and without sorting: doubled, a new bucket
    /// holds what one old bucket did, its chain reversed; halved, bucket
    /// `b` holds old bucket `b`'s chain reversed, then above it old bucket
    /// `b + size`'s reversed. False (nothing done) for any other resize or
    /// when the numbers would not fit.
    fn strt_rehash_halves(&mut self, from: usize, size: usize, skip: *mut LuaStr) -> bool {
        let grow = size == 2 * from;
        if !(grow || 2 * size == from) || self.strt.seq.len() != from || from == 0 {
            return false;
        }
        let old = std::mem::take(&mut self.strt.seq);
        let mut seq = std::mem::take(&mut self.strt.spare);
        seq.clear();
        // each bound one above the highest number a string may get (a
        // string never numbered, 0, ends up above the others)
        if grow {
            for b in 0..size {
                match old[b & (from - 1)].checked_add(1) {
                    Some(n) => seq.push(n),
                    None => {
                        self.strt.seq = old;
                        self.strt.spare = seq;
                        return false;
                    }
                }
            }
        } else {
            for b in 0..size {
                match old[b]
                    .checked_add(old[b + size])
                    .and_then(|n| n.checked_add(2))
                {
                    Some(n) => seq.push(n),
                    None => {
                        self.strt.seq = old;
                        self.strt.spare = seq;
                        return false;
                    }
                }
            }
        }
        let mut v = std::mem::take(&mut self.strt.order);
        v.clear();
        self.strings.each(|p| v.push((0, 0, p)));
        for &(_, _, p) in &v {
            if p == skip || !self.puc_holds(p) {
                continue;
            }
            let b = self.puc_bucket(p, from);
            // SAFETY: the table holds allocated strings
            let s = unsafe { Gc::from_ptr(p) };
            let mut k = old[b] + 1 - s.chain();
            if !grow && b >= size {
                k += old[b - size] + 1;
            }
            s.set_chain(k);
        }
        self.strt.order = v;
        self.strt.seq = seq;
        self.strt.spare = old;
        true
    }

    /// The bucket `b` numbered again from 1, in its order.
    #[cold]
    fn strt_renumber(&mut self, b: usize) {
        let size = self.puc.strt_size;
        let mut v = Vec::new();
        self.strings.each(|p| v.push(p));
        v.retain(|&p| self.puc_holds(p) && self.puc_bucket(p, size) == b);
        // SAFETY: the table holds allocated strings
        v.sort_unstable_by_key(|&p| unsafe { Gc::from_ptr(p).chain() });
        for (i, &p) in v.iter().enumerate() {
            // SAFETY: as above
            unsafe { Gc::from_ptr(p).set_chain(i as u16 + 1) };
        }
        self.strt.seq[b] = v.len() as u16;
    }

    /// String `p` at the head of its chain in a table of `size` buckets.
    fn strt_head(&mut self, p: *mut LuaStr, size: usize) {
        let b = self.puc_bucket(p, size);
        if self.strt.seq.len() != size {
            self.strt.seq.resize(size, 0);
        }
        if self.strt.seq[b] == u16::MAX {
            self.strt_renumber(b);
        }
        self.strt.seq[b] += 1;
        // SAFETY: `p` is an allocated string
        unsafe { Gc::from_ptr(p).set_chain(self.strt.seq[b]) };
    }

    /// The counted string `h` entered PUC's string table, which may have
    /// grown on the way (`grow_from`): 5.1 puts it in, then resizes, 5.2
    /// resizes first.
    pub(crate) fn strt_enter(&mut self, h: *mut GcHeader) {
        if !cfg!(feature = "gc-trace") || !matches!(self.table_dialect, Dialect::L51 | Dialect::L52)
        {
            return;
        }
        let p = h as *mut LuaStr;
        let size = self.puc.strt_size;
        if !self.puc_holds(p) {
            if let Some(from) = self.strt.grow_from.take() {
                self.strt_reorder(from, size, std::ptr::null_mut());
            }
            return;
        }
        match self.strt.grow_from.take() {
            Some(from) if self.table_dialect == Dialect::L51 => {
                self.strt_head(p, from);
                self.strt_reorder(from, size, std::ptr::null_mut());
            }
            Some(from) => {
                self.strt_reorder(from, size, p);
                self.strt_head(p, size);
            }
            None => self.strt_head(p, size),
        }
    }

    /// PUC's string table resized from `from` buckets with no new string
    /// on the way (a shrink, or a string luna has no object for).
    pub(crate) fn strt_resized(&mut self, from: usize) {
        if cfg!(feature = "gc-trace") && matches!(self.table_dialect, Dialect::L51 | Dialect::L52) {
            self.strt.grow_from = None;
            self.strt_reorder(from, self.puc.strt_size, std::ptr::null_mut());
        }
    }

    /// The table grew from `from` buckets for a string about to enter.
    pub(crate) fn strt_grew(&mut self, from: usize) {
        if cfg!(feature = "gc-trace") {
            self.strt.grow_from = Some(from);
        }
    }

    /// A string luna has no object for entered: a growth on its way is
    /// done.
    pub(crate) fn strt_settle(&mut self) {
        if let Some(from) = self.strt.grow_from.take() {
            self.strt_resized(from);
        }
    }
}
