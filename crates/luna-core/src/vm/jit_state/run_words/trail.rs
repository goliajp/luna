//! The trail of links taken in a row: kept in the run words, where the
//! code that takes a link itself writes it (see `RunWords::trail_at`).

use super::*;

impl RunWords {
    /// The number of links on the trail.
    #[inline]
    pub(crate) fn trail_len(&self) -> usize {
        self.trail_len.get() as usize
    }

    /// The link at `i` of the trail.
    pub(crate) fn trail_get(&self, i: usize) -> [u32; 3] {
        debug_assert!(i < self.trail_len());
        self.trail[i]
    }

    /// Notes a link, growing the trail when it is full.
    pub(crate) fn trail_push(&mut self, link: [u32; 3]) -> Result<(), Oom> {
        let n = self.trail_len();
        if n == self.trail.len() {
            self.trail_reserve(1)?;
        }
        self.trail[n] = link;
        self.trail_len.set(n as u32 + 1);
        Ok(())
    }

    /// Forgets the links from `n` on.
    pub(crate) fn trail_truncate(&mut self, n: usize) {
        if n < self.trail_len() {
            self.trail_len.set(n as u32);
        }
    }

    /// Room for `extra` links past those on the trail.
    pub(crate) fn trail_reserve(&mut self, extra: usize) -> Result<(), Oom> {
        let want = self.trail_len() + extra;
        if want <= self.trail.len() {
            return Ok(());
        }
        self.trail.reserve(want - self.trail.len())?;
        while self.trail.len() < want {
            self.trail.push([0; 3])?;
        }
        self.trail_at.set(self.trail.as_ptr().cast());
        self.trail_cap.set(self.trail.len() as u32);
        Ok(())
    }

    /// Where the innermost dispatch's links start in the trail.
    #[inline]
    pub(crate) fn link_base(&self) -> usize {
        self.link_base.get() as usize
    }

    #[inline]
    pub(crate) fn set_link_base(&self, n: usize) {
        self.link_base.set(n as u32);
    }
}
