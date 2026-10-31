//! The records of objects PUC makes and luna does not (yet): counted
//! when PUC makes them, handed over when luna makes the object after all.

use super::super::*;
use crate::runtime::mem::Oom;
use crate::runtime::puc::records::ghost::*;

impl Heap {
    /// A table PUC makes, `a` array slots and `h` nodes, and luna does not
    /// (yet): counted now, freed by the next sweep unless luna makes it
    /// after all ([`Heap::ghost_table_made`]).
    pub(crate) fn acct_ghost_table(&mut self, a: usize, h: usize) -> Result<GhostRef, Oom> {
        // made while counting is set aside: an entry of no bytes, nothing
        // counted for it or freed
        let size = [self.puc.sizes.table as u64, 0][usize::from(!self.counting())];
        let (g, at) = self.ghost_push(size)?;
        // room for the two parts first: no half record is left behind
        // SAFETY: `ghost_push` just returned `g`, a live record of this heap
        let sizes = unsafe { &mut (*g).sizes };
        if let Err(e) = sizes.reserve(2) {
            sizes.pop();
            return Err(e);
        }
        self.ghost_push(PART)?;
        self.ghost_push(PART)?;
        self.acct_obj(PucKind::Table, self.puc.sizes.table);
        let t = GhostRef { g, at };
        self.ghost_table_resize(t, 0, 0, a, h);
        Ok(t)
    }

    /// The table of `t` resized from `old_a` array slots and `old_h` nodes
    /// to `new_a` and `new_h` (PUC `luaH_resize`).
    pub(crate) fn ghost_table_resize(
        &mut self,
        t: GhostRef,
        old_a: usize,
        old_h: usize,
        new_a: usize,
        new_h: usize,
    ) {
        // counted as the table is (an entry of no bytes is not)
        if self.record(t)[t.at] & !FLAGS == 0 {
            return;
        }
        self.acct_counted_table_resize((old_a, old_h), (new_a, new_h));
        let (ab, hb) = (
            self.array_bytes(new_a) as u64,
            self.hash_bytes(new_h) as u64,
        );
        let sizes = self.record(t);
        sizes[t.at + 1] = PART | ab;
        sizes[t.at + 2] = PART | hb;
    }

    /// PUC's bytes of a new table of `a` array slots and `h` nodes.
    pub(crate) fn puc_table_bytes(&self, a: usize, h: usize) -> usize {
        self.puc.sizes.table + self.array_bytes(a) + self.hash_bytes(h)
    }

    /// luna made `h`, without counting, for the object of `r` after all
    /// (one without parts): the record neither frees nor sweeps it, and
    /// `h` is counted from now on.
    pub(crate) fn ghost_made(&mut self, r: GhostRef, h: Gc<GcHeader>) {
        self.record(r)[r.at] |= TAKEN;
        // SAFETY: `h` is a handle, so its object is live; nothing borrows
        // the header while its per-type byte is written
        unsafe { (*h.as_ptr()).set_uncounted(false) };
    }

    /// luna made `h`, without counting, for the table of `t` after all (a
    /// trace leaving with it): the record neither frees nor sweeps it, and
    /// `h` is counted from now on, as it grows and when it is freed (made
    /// uncounted, it was one of luna's own tables).
    pub(crate) fn ghost_table_made(&mut self, t: GhostRef, h: Gc<GcHeader>) {
        for e in &mut self.record(t)[t.at..t.at + 3] {
            *e |= TAKEN;
        }
        // SAFETY: as in `ghost_made`
        unsafe {
            (*h.as_ptr()).set_uncounted(false);
            (*h.as_ptr()).cls &= !UNLISTED;
        }
    }
}
