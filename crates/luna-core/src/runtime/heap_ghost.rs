//! Records of objects PUC makes and luna does not (the boxes of
//! `luaL_Buffer`, the strings a concatenation makes on the way, the
//! parser's anchor table): counted when PUC makes them, garbage from the
//! start, freed at the next sweep as PUC frees them. A record sits on the
//! object list like any object; consecutive ones share a record.

use super::*;
use crate::runtime::mem::{LVec, Oom};

/// A run of objects PUC made and luna did not: their sizes, in the order
/// PUC made them.
#[repr(C)]
pub(crate) struct Ghost {
    pub(crate) hdr: GcHeader,
    pub(crate) sizes: LVec<u32>,
}

impl Heap {
    /// An object of `kind` and `size` bytes PUC makes and luna does not:
    /// counted now, freed by the next sweep.
    pub(crate) fn acct_ghost(&mut self, kind: PucKind, size: usize) -> Result<(), Oom> {
        if self.puc.silent > 0 {
            return Ok(());
        }
        self.acct_obj(kind, size);
        let born = if self.phase == GcPhase::Propagate {
            BLACK
        } else {
            self.current_white
        };
        let head = self.all;
        // SAFETY: `head`, when not null, heads a live object of this heap
        if !head.is_null()
            && unsafe { (*head).tag == ObjTag::Ghost && (*head).flags & COLOR_BITS == born }
        {
            // SAFETY: a header tagged `Ghost` is a `Ghost`, and nothing else
            // refers into it
            return unsafe { (*(head as *mut Ghost)).sizes.push(size as u32) };
        }
        let mut sizes = LVec::new(self.mem());
        sizes.push(size as u32)?;
        self.try_adopt(Ghost {
            hdr: GcHeader::new(ObjTag::Ghost),
            sizes,
        })?;
        Ok(())
    }

    /// A record of PUC's objects freed: each one's free counted.
    ///
    /// # Safety
    /// `g` heads an unlinked record of this heap that nothing uses
    /// afterwards.
    pub(super) unsafe fn free_ghost(&mut self, g: *mut Ghost) {
        // SAFETY: the caller's contract
        unsafe {
            for &s in (*g).sizes.iter() {
                self.acct_free(s as usize);
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
