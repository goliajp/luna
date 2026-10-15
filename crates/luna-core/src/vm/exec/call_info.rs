//! PUC 5.2+'s `CallInfo`s: one per call level, allocated the first time a
//! thread reaches the level (`luaE_extendCI`) and kept until the collector
//! frees the unused ones. Only the count is kept, for what they take of
//! `totalbytes`.

use super::*;

impl Vm {
    /// The `CallInfo`s the running thread uses besides its base one: its
    /// frames as 5.1 counts them in its array, less the array's first
    /// entry. On the main thread that entry is the protected call the
    /// standalone interpreter makes its script's (`docall`'s `lua_pcall`
    /// takes no level of PUC's), with the host's frames below it.
    #[inline(always)]
    pub(super) fn ci_in_use(&self) -> u32 {
        self.frames_in_use().saturating_sub(1)
    }

    /// A call level was entered: counted when the thread has not been this
    /// deep before. `ci_alloc` stays at its maximum in 5.1, which counts its
    /// array where it grows (`grow_frames`).
    #[inline(always)]
    pub(super) fn note_call_level(&mut self) {
        if self.ci_in_use() > self.g.ci_alloc {
            self.extend_ci();
        }
    }

    #[cold]
    #[inline(never)]
    fn extend_ci(&mut self) {
        let n = self.ci_in_use();
        self.extend_ci_to(n);
    }

    /// The running thread has `n` `CallInfo`s at least.
    pub(super) fn extend_ci_to(&mut self, n: u32) {
        while self.g.ci_alloc < n {
            self.heap.acct_ci(1, 0);
            self.g.ci_alloc += 1;
        }
    }
}
