//! Keepers: a long string of PUC's that luna holds no object for, which
//! lives as long as one of luna's objects (the default package path, which
//! luna's package table stands for). A keeper is a record on the object
//! list made where PUC makes the string, so that the sweep frees the
//! string where PUC's sweep does; it lives while its host does.

use super::*;
use crate::runtime::mem::{LVec, Oom};

/// In a record's `sub` byte: a keeper.
const KEEPER: u8 = 0x10;

impl Heap {
    /// A long string of `size` bytes PUC makes now, living as long as the
    /// object behind `host`: counted now, and freed where PUC frees it.
    pub(in crate::runtime) fn acct_kept_string(
        &mut self,
        host: Gc<GcHeader>,
        size: usize,
    ) -> Result<(), Oom> {
        let host = host.as_ptr();
        let mut sizes = LVec::new(self.mem());
        sizes.push(size as u64)?;
        self.puc
            .records
            .keepers
            .try_reserve(1)
            .map_err(|_| Oom(self.mem()))?;
        let mut hdr = GcHeader::new(ObjTag::Ghost);
        hdr.sub = KEEPER;
        // `kept_dead` asks about it at every sweep
        hdr.cls |= parse::KEPT;
        let g = self.try_adopt(Ghost { hdr, sizes })?;
        self.acct_obj(PucKind::LongStr, size);
        self.puc
            .records
            .keepers
            .push((g.as_ptr() as *mut GcHeader, host));
        // the host's traversal counts the string as PUC's marking does
        // SAFETY: `host` heads a live object; only its class byte is written
        unsafe { (*host).cls |= STANDS };
        Ok(())
    }

    /// The bytes of the strings kept for `host`, which PUC's marking of
    /// it reaches.
    pub(in crate::runtime::puc) fn kept_bytes(&self, host: *mut GcHeader) -> usize {
        kept_bytes(&self.puc.records.keepers, host)
    }

    /// The sweep reaches the dead keeper `h`: true (and current white)
    /// while its host lives, which the sweep has not reached yet (older);
    /// otherwise it is no keeper any more, and freed as a record.
    ///
    /// # Safety
    /// `h` heads a record of this heap.
    pub(in crate::runtime::puc) unsafe fn keeper_kept(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract; a host lives until the sweep
        // passes it, after its keeper
        unsafe {
            if (*h).tag != ObjTag::Ghost || (*h).sub != KEEPER {
                return false;
            }
            let Some(i) = self.puc.records.keepers.iter().position(|k| k.0 == h) else {
                return false;
            };
            let f = (*self.puc.records.keepers[i].1).flags();
            if is_white(f) && f & self.white() == 0 {
                self.puc.records.keepers.swap_remove(i);
                return false;
            }
            (*h).paint(self.white());
            true
        }
    }
}

/// [`Heap::kept_bytes`] of the keepers `ks`.
pub(in crate::runtime::puc) fn kept_bytes(
    ks: &[(*mut GcHeader, *mut GcHeader)],
    host: *mut GcHeader,
) -> usize {
    let ks = ks.iter().filter(|k| k.1 == host);
    // SAFETY: a keeper is a live record until it is unlisted
    ks.map(|k| unsafe { (*(k.0 as *const Ghost)).sizes.iter().sum::<u64>() } as usize)
        .sum()
}
