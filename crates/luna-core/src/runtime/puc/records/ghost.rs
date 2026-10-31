//! Records of objects PUC makes and luna does not (the boxes of
//! `luaL_Buffer`, the strings a concatenation makes on the way, the
//! parser's anchor table): counted when PUC makes them, garbage from the
//! start, freed at the next sweep as PUC frees them. A record sits on the
//! object list like any object; consecutive ones share a record.

use super::*;
use crate::runtime::mem::{LVec, Oom};

/// A run of objects PUC made and luna did not: their sizes, in the order
/// PUC made them, a short string's with [`SHORT`] set (its free leaves
/// the string table too); a table's header followed by its array and
/// hash parts, each marked [`PART`].
#[repr(C)]
pub(crate) struct Ghost {
    pub(in crate::runtime) hdr: GcHeader,
    pub(in crate::runtime) sizes: LVec<u64>,
}

pub(in crate::runtime::puc) const SHORT: u64 = 1 << 63;
/// a part freed with the object before it, not an object of its own
pub(in crate::runtime::puc) const PART: u64 = 1 << 62;
/// an object luna made after all, which stands for it: neither freed nor
/// swept here
pub(in crate::runtime::puc) const TAKEN: u64 = 1 << 61;
pub(in crate::runtime::puc) const FLAGS: u64 = SHORT | PART | TAKEN;

/// Where an object's record is (its record and the object's place in
/// it), while luna holds no object for it (a trace's sunk table or the
/// string of its accumulated concatenation).
#[derive(Clone, Copy)]
pub(crate) struct GhostRef {
    pub(in crate::runtime::puc) g: *mut Ghost,
    pub(in crate::runtime::puc) at: usize,
}

impl Heap {
    /// An object of `kind` and `size` bytes PUC makes and luna does not:
    /// counted now, freed by the next sweep.
    pub(in crate::runtime::puc) fn acct_ghost(
        &mut self,
        kind: PucKind,
        size: usize,
    ) -> Result<(), Oom> {
        if !self.counting() {
            return Ok(());
        }
        // the record first: a retry after it failed counts the object once
        self.ghost_record(kind, size)?;
        self.acct_obj(kind, size);
        Ok(())
    }

    /// A record of an object PUC made whose allocation was counted
    /// already.
    fn ghost_record(&mut self, kind: PucKind, size: usize) -> Result<(), Oom> {
        let size = size as u64 | if kind == PucKind::ShortStr { SHORT } else { 0 };
        self.ghost_push(size).map(drop)
    }

    /// `entry` at the end of the newest record (a new one when the newest
    /// object is not a record); the record and the entry's place.
    pub(in crate::runtime::puc) fn ghost_push(
        &mut self,
        entry: u64,
    ) -> Result<(*mut Ghost, usize), Oom> {
        let size = entry;
        let born = self.white();
        let head = self.list_head();
        let ghost_head = !head.is_null()
            // SAFETY: `head`, not null, heads a live object of this heap
            && unsafe {
                (*head).tag == ObjTag::Ghost && (*head).sub == 0 && (*head).flags() & COLOR_BITS == born
            };
        if ghost_head {
            let g = head as *mut Ghost;
            // SAFETY: a header tagged `Ghost` is a `Ghost`, and nothing else
            // refers into it
            let sizes = unsafe { &mut (*g).sizes };
            sizes.push(size)?;
            return Ok((g, sizes.len() - 1));
        }
        let mut sizes = LVec::new(self.mem());
        sizes.push(size)?;
        let g = self.try_adopt(Ghost {
            hdr: GcHeader::new(ObjTag::Ghost),
            sizes,
        })?;
        Ok((g.as_ptr(), 0))
    }

    /// The entries of the record `r` is in.
    pub(in crate::runtime::puc) fn record(&mut self, r: GhostRef) -> &mut LVec<u64> {
        // SAFETY: a record lives until the sweep after the trace that made
        // it has left, and its entries are reached only through here, one
        // borrow at a time
        unsafe { &mut (*r.g).sizes }
    }

    /// An object of `kind` and `size` bytes PUC makes and luna does not
    /// (yet), as [`Heap::acct_ghost`]: where its record is.
    pub(in crate::runtime::puc) fn acct_ghost_ref(
        &mut self,
        kind: PucKind,
        size: usize,
    ) -> Result<GhostRef, Oom> {
        // made while counting is set aside: an entry of no bytes
        let size = size * usize::from(self.counting());
        let size64 = size as u64 | if kind == PucKind::ShortStr { SHORT } else { 0 };
        let (g, at) = self.ghost_push(size64)?;
        self.acct_obj(kind, size);
        Ok(GhostRef { g, at })
    }

    /// The objects of PUC's a record holds, as a sweep counts them.
    pub(in crate::runtime::puc) fn ghost_objects(g: &Ghost) -> i64 {
        g.sizes.iter().filter(|&&s| s & (PART | TAKEN) == 0).count() as i64
    }

    /// The free of a record's entry `s`: the bytes, a short string's place
    /// in the string table; nothing for an empty part or one luna took.
    fn ghost_free(&mut self, s: u64) {
        if s & TAKEN != 0 || s & !FLAGS == 0 {
            return;
        }
        if s & SHORT != 0 {
            self.puc.records.strt_nuse -= 1;
        }
        self.acct_part((s & !FLAGS) as usize, 0);
    }

    /// Free up to `quota` of the newest objects the dead record `g` holds
    /// (a sweep meets them newest first); how many, and whether none is
    /// left (the caller then unlinks and frees the record).
    pub(in crate::runtime::puc) fn free_ghost_records(
        &mut self,
        g: &mut Ghost,
        quota: i64,
    ) -> (i64, bool) {
        let mut n = 0;
        while n < quota {
            let Some(s) = g.sizes.pop() else { break };
            self.ghost_free(s);
            n += i64::from(s & (PART | TAKEN) == 0);
        }
        (n, g.sizes.is_empty())
    }

    /// A record of PUC's objects freed: each one's free counted.
    ///
    /// # Safety
    /// `g` heads an unlinked record of this heap that nothing uses
    /// afterwards.
    pub(in crate::runtime::puc) unsafe fn free_ghost(&mut self, g: *mut Ghost) {
        // SAFETY: the caller's contract
        unsafe {
            let sizes = &(*g).sizes;
            for &s in sizes.iter() {
                self.ghost_free(s);
            }
            self.free_block(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::heap::Heap;
    use crate::runtime::puc::PucKind;

    fn heap() -> Heap {
        let mut h = Heap::new();
        h.set_version(crate::version::LuaVersion::Lua54).unwrap();
        h
    }

    #[test]
    fn records_count_when_made_and_free_at_the_next_collection() {
        let mut h = heap();
        h.collect(&[]);
        let before = h.bytes();
        h.acct_ghost(PucKind::Userdata, 48).unwrap();
        h.acct_ghost(PucKind::LongStr, 100).unwrap();
        assert_eq!(h.bytes(), before + 148);
        // consecutive records share one object
        let live = h.live_objects();
        h.acct_ghost(PucKind::Userdata, 48).unwrap();
        assert_eq!(h.live_objects(), live);
        h.collect(&[]);
        assert_eq!(h.bytes(), before);
    }

    #[test]
    fn an_object_between_records_starts_a_new_one() {
        let mut h = heap();
        h.acct_ghost(PucKind::Userdata, 48).unwrap();
        let live = h.live_objects();
        let _t = h.new_table();
        h.acct_ghost(PucKind::Userdata, 48).unwrap();
        assert_eq!(h.live_objects(), live + 2);
    }
}
