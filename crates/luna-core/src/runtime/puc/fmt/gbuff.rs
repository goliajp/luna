//! 5.1 and 5.2's buffer for concatenations (`G(L)->buff`).

use super::*;
use crate::runtime::mem::{LVec, Oom};

/// The buffer: a block of its size, which the allocation function sees
/// grow, shrink and go as PUC's does, and may refuse.
#[derive(Default)]
pub(crate) struct Gbuff(Option<LVec<u8>>);

impl Gbuff {
    /// `buff->buffsize`
    pub(crate) fn size(&self) -> usize {
        self.0.as_ref().map_or(0, LVec::capacity)
    }
}

impl Heap {
    /// 5.1 and 5.2's buffer for concatenations made room for `n` bytes
    /// (`luaZ_openspace`, at least `LUA_MINBUFFER`).
    pub(crate) fn open_gbuff(&mut self, n: usize) -> Result<(), Oom> {
        let old = self.puc.gbuff.size();
        if n > old {
            let mem = self.mem();
            let size = n.max(32);
            let b = self.puc.gbuff.0.get_or_insert_with(|| LVec::new(mem));
            b.reserve_exact(size)?;
            let waited = self.refusal_waits();
            self.acct_part(old, size);
            // gc-trace: `LUNA_GC_REFUSE` refused this growth of PUC's, which is
            // then as if the allocation failed
            if let Some(bytes) = self.take_new_refusal(waited) {
                self.puc.bytes = (self.puc.bytes as i64 - bytes) as usize;
                self.shrink_gbuff_to(old);
                return Err(Oom(mem));
            }
        }
        Ok(())
    }

    /// The buffer set to `size` bytes by `checkSizes`, smaller (5.1 halves
    /// it, 5.2 frees it).
    pub(crate) fn shrink_gbuff(&mut self, size: usize) {
        let old = self.puc.gbuff.size();
        self.shrink_gbuff_to(size);
        self.acct_part(old, self.puc.gbuff.size());
    }

    /// The buffer's block made `size` bytes, smaller, uncounted.
    fn shrink_gbuff_to(&mut self, size: usize) {
        let Some(b) = self.puc.gbuff.0.as_mut() else {
            return;
        };
        if size == 0 {
            self.puc.gbuff.0 = None;
        } else {
            // the bytes in use decide what `shrink_to_fit` leaves; within
            // the block, nothing is allocated
            b.resize(size, 0).expect("smaller than the block");
            b.shrink_to_fit();
            b.clear();
        }
    }
}
