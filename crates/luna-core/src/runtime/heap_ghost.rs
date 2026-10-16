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
    pub(crate) hdr: GcHeader,
    pub(crate) sizes: LVec<u32>,
}

const SHORT: u32 = 1 << 31;
/// a part freed with the object before it, not an object of its own
const PART: u32 = 1 << 30;
/// an object luna made after all, which stands for it: neither freed nor
/// swept here
const TAKEN: u32 = 1 << 29;
const FLAGS: u32 = SHORT | PART | TAKEN;

/// Where an object's record is (its record and the object's place in
/// it), while luna holds no object for it (a trace's sunk table or the
/// string of its accumulated concatenation).
#[derive(Clone, Copy)]
pub(crate) struct GhostRef {
    g: *mut Ghost,
    at: usize,
}

impl Heap {
    /// An object of `kind` and `size` bytes PUC makes and luna does not:
    /// counted now, freed by the next sweep.
    pub(crate) fn acct_ghost(&mut self, kind: PucKind, size: usize) -> Result<(), Oom> {
        if self.puc.silent > 0 {
            return Ok(());
        }
        // the record first: a retry after it failed counts the object once
        self.ghost_record(kind, size)?;
        self.acct_obj(kind, size);
        Ok(())
    }

    /// A record of an object PUC made whose allocation was counted
    /// already.
    pub(crate) fn ghost_record(&mut self, kind: PucKind, size: usize) -> Result<(), Oom> {
        let size = size as u32 | if kind == PucKind::ShortStr { SHORT } else { 0 };
        self.ghost_push(size).map(drop)
    }

    /// `entry` at the end of the newest record (a new one when the newest
    /// object is not a record); the record and the entry's place.
    fn ghost_push(&mut self, entry: u32) -> Result<(*mut Ghost, usize), Oom> {
        let size = entry;
        let born = self.current_white;
        let head = self.all;
        let ghost_head = !head.is_null()
            // SAFETY: `head`, not null, heads a live object of this heap
            && unsafe { (*head).tag == ObjTag::Ghost && (*head).flags & COLOR_BITS == born };
        if ghost_head {
            let g = head as *mut Ghost;
            // SAFETY: a header tagged `Ghost` is a `Ghost`, and nothing else
            // refers into it
            unsafe { (*g).sizes.push(size)? };
            // SAFETY: as above
            return Ok((g, unsafe { (*g).sizes.len() } - 1));
        }
        let mut sizes = LVec::new(self.mem());
        sizes.push(size)?;
        let g = self.try_adopt(Ghost {
            hdr: GcHeader::new(ObjTag::Ghost),
            sizes,
        })?;
        Ok((g.as_ptr(), 0))
    }

    /// A table PUC makes, `a` array slots and `h` nodes, and luna does not
    /// (yet): counted now, freed by the next sweep unless luna makes it
    /// after all ([`Heap::ghost_table_taken`]).
    pub(crate) fn acct_ghost_table(&mut self, a: usize, h: usize) -> Result<GhostRef, Oom> {
        let (g, at) = self.ghost_push(self.sizes.table as u32)?;
        // room for the two parts first: no half record is left behind
        // SAFETY: `ghost_push` just returned `g`, a live record of this heap
        if let Err(e) = unsafe { (*g).sizes.reserve(2) } {
            // SAFETY: as above
            unsafe { (*g).sizes.pop() };
            return Err(e);
        }
        self.ghost_push(PART)?;
        self.ghost_push(PART)?;
        self.acct_obj(PucKind::Table, self.sizes.table);
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
        self.acct_table_resize(old_a, old_h, new_a, new_h);
        let (ab, hb) = (
            self.array_bytes(new_a) as u32,
            self.hash_bytes(new_h) as u32,
        );
        // SAFETY: a ghost table's record lives until the sweep after it,
        // and a sweep comes only after the trace that made it has left
        unsafe {
            let sizes = &mut (*t.g).sizes;
            sizes[t.at + 1] = PART | ab;
            sizes[t.at + 2] = PART | hb;
        }
    }

    /// PUC's bytes of a new table of `a` array slots and `h` nodes.
    pub(crate) fn puc_table_bytes(&self, a: usize, h: usize) -> usize {
        self.sizes.table + self.array_bytes(a) + self.hash_bytes(h)
    }

    /// The object behind `h`, made without counting, stands for one of
    /// PUC's counted already: counted from now on.
    pub(crate) fn set_counted(&mut self, h: *mut GcHeader) {
        super::set_uncounted(h, false);
    }

    /// An object of `kind` and `size` bytes PUC makes and luna does not
    /// (yet), as [`Heap::acct_ghost`]: where its record is.
    pub(crate) fn acct_ghost_ref(&mut self, kind: PucKind, size: usize) -> Result<GhostRef, Oom> {
        let size32 = size as u32 | if kind == PucKind::ShortStr { SHORT } else { 0 };
        let (g, at) = self.ghost_push(size32)?;
        self.acct_obj(kind, size);
        Ok(GhostRef { g, at })
    }

    /// luna made the object of `r` after all (one without parts): its
    /// record neither frees nor sweeps it.
    pub(crate) fn ghost_taken(&mut self, r: GhostRef) {
        // SAFETY: as in `ghost_table_resize`
        unsafe {
            let sizes = &mut (*r.g).sizes;
            sizes[r.at] |= TAKEN;
        }
    }

    /// luna made the table of `t` after all (a trace leaving with it): its
    /// record neither frees nor sweeps it.
    pub(crate) fn ghost_table_taken(&mut self, t: GhostRef) {
        // SAFETY: as in `ghost_table_resize`
        unsafe {
            let sizes = &mut (*t.g).sizes;
            for e in &mut sizes[t.at..t.at + 3] {
                *e |= TAKEN;
            }
        }
    }

    /// The objects of PUC's a record holds, as a sweep counts them.
    ///
    /// # Safety
    /// `g` heads a record of this heap.
    pub(in crate::runtime::heap) unsafe fn ghost_objects(&self, g: *mut Ghost) -> i64 {
        // SAFETY: the caller's contract
        unsafe {
            let sizes = &(*g).sizes;
            sizes.iter().filter(|&&s| s & (PART | TAKEN) == 0).count() as i64
        }
    }

    /// The free of a record's entry `s`: the bytes, a short string's place
    /// in the string table; nothing for an empty part or one luna took.
    fn ghost_free(&mut self, s: u32) {
        if s & TAKEN != 0 || s & !FLAGS == 0 {
            return;
        }
        if s & SHORT != 0 {
            self.puc.strt_nuse -= 1;
        }
        self.acct_free((s & !FLAGS) as usize);
    }

    /// Free up to `quota` of the newest objects the dead record `g` holds
    /// (a sweep meets them newest first); how many, and whether none is
    /// left (the caller then unlinks and frees the record).
    ///
    /// # Safety
    /// `g` heads a dead record of this heap.
    pub(in crate::runtime::heap) unsafe fn free_ghost_records(
        &mut self,
        g: *mut Ghost,
        quota: i64,
    ) -> (i64, bool) {
        let mut n = 0;
        // SAFETY: the caller's contract
        unsafe {
            while n < quota {
                let Some(s) = (*g).sizes.pop() else { break };
                self.ghost_free(s);
                n += i64::from(s & (PART | TAKEN) == 0);
            }
            (n, (*g).sizes.is_empty())
        }
    }

    /// `gc-verify`: the short strings the live records hold, each a place
    /// in PUC's string table.
    #[cfg(feature = "gc-verify")]
    pub(in crate::runtime::heap) fn ghost_short_strings(&self) -> usize {
        let mut n = 0;
        let mut cur = self.all;
        // SAFETY: `all` links this heap's live objects; a header tagged
        // `Ghost` is a `Ghost`
        unsafe {
            while !cur.is_null() {
                if (*cur).tag == ObjTag::Ghost {
                    let sizes = &(*(cur as *const Ghost)).sizes;
                    n += sizes
                        .iter()
                        .filter(|&&s| s & (SHORT | TAKEN) == SHORT)
                        .count();
                }
                cur = (*cur).next;
            }
        }
        n
    }

    /// A record of PUC's objects freed: each one's free counted.
    ///
    /// # Safety
    /// `g` heads an unlinked record of this heap that nothing uses
    /// afterwards.
    pub(in crate::runtime::heap) unsafe fn free_ghost(&mut self, g: *mut Ghost) {
        // SAFETY: the caller's contract
        unsafe {
            let n = (&(*g).sizes).len();
            for i in 0..n {
                let s = (&(*g).sizes)[i];
                self.ghost_free(s);
            }
            self.free_block(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::heap::{Heap, PucKind};

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
