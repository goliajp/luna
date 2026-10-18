//! The order of each chain of PUC's string table (5.1, 5.2), which the
//! string sweep frees a bucket's dead strings in: a new string goes to
//! the head of its chain, and a resize (`luaS_resize`) takes the old
//! buckets in order, each chain from its head, putting each string at the
//! head of its new chain. A string's place is a number in its header
//! (`LuaStr::chain`), the newest of a chain highest; a bucket that runs
//! out of numbers is numbered again from 1.

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
    /// chain.
    fn strt_reorder(&mut self, from: usize, size: usize, skip: *mut LuaStr) {
        let mut all = Vec::new();
        self.strings.each(|p| all.push(p));
        let mut v: Vec<(usize, u16, *mut LuaStr)> = all
            .into_iter()
            .filter(|&p| p != skip && self.puc_holds(p))
            // SAFETY: the table holds allocated strings
            .map(|p| {
                (
                    self.puc_bucket(p, from),
                    unsafe { Gc::from_ptr(p).chain() },
                    p,
                )
            })
            .collect();
        // the old buckets in order, each chain from its head (newest)
        v.sort_unstable_by_key(|&(b, k, _)| (b, std::cmp::Reverse(k)));
        let seq = &mut self.strt.seq;
        seq.clear();
        seq.resize(size, 0);
        for (_, _, p) in v {
            let b = self.puc_bucket(p, size);
            let seq = &mut self.strt.seq;
            seq[b] += 1;
            // SAFETY: as above
            unsafe { Gc::from_ptr(p).set_chain(seq[b]) };
        }
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
        if !matches!(self.table_dialect, Dialect::L51 | Dialect::L52) {
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
        if matches!(self.table_dialect, Dialect::L51 | Dialect::L52) {
            self.strt.grow_from = None;
            self.strt_reorder(from, self.puc.strt_size, std::ptr::null_mut());
        }
    }

    /// The table grew from `from` buckets for a string about to enter.
    pub(crate) fn strt_grew(&mut self, from: usize) {
        self.strt.grow_from = Some(from);
    }

    /// A string luna has no object for entered: a growth on its way is
    /// done.
    pub(crate) fn strt_settle(&mut self) {
        if let Some(from) = self.strt.grow_from.take() {
            self.strt_resized(from);
        }
    }
}
