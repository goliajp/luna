//! PUC 5.2+'s `CallInfo`s: one per call level, allocated the first time a
//! thread reaches the level (`luaE_extendCI`) and kept until the collector
//! frees the unused ones. Only the count is kept, for what they take of
//! `totalbytes`.

use super::*;

impl Vm {
    /// The `CallInfo`s the running thread uses besides its base one: its
    /// frames as 5.1 counts them in its array, less a coroutine's base
    /// `CallInfo` and the host's protected calls on the main thread (which
    /// counts no base: the host's frames are below its own).
    #[inline(always)]
    pub(super) fn ci_in_use(&self) -> u32 {
        let base = match self.current {
            Some(_) => 1,
            None => self.g.host_pcalls,
        };
        self.frames_in_use() - base
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

    /// PUC calls a C function one level above the running one, which luna
    /// does not call (a `luaL_Buffer` box's `__close`).
    pub(crate) fn note_c_call(&mut self) {
        self.note_c_calls(1);
    }

    /// PUC has `n` C functions' `CallInfo`s above the running one.
    pub(crate) fn note_c_calls(&mut self, n: u32) {
        let n = self.ci_in_use() + n;
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

impl Vm {
    /// Whether the calls a compiled trace inlines (`depth` levels deep,
    /// its registers up to `base + window`) fit what the thread has: PUC
    /// would make them `CallInfo`s and stack where it has not, counted
    /// and (5.3, 5.4) checked for a collection, so the interpreter runs
    /// the loop until it has.
    pub(super) fn inlined_calls_fit(&self, depth: u8, base: u32, window: u32) -> bool {
        if depth == 0 {
            return true;
        }
        let d = u32::from(depth);
        let levels = if self.version == LuaVersion::Lua51 {
            self.frames_in_use() + d <= self.g.frame_size
        } else {
            self.ci_in_use() + d <= self.g.ci_alloc
        };
        levels && base + window < self.g.stack_last
    }
}

impl Vm {
    /// Which C stack the running C function is on, as far as where its
    /// frame lies: the C calls below it (the native functions running, the
    /// protected and metamethod calls each Lua frame made) and their
    /// number. Two calls with the same key have their locals at the same
    /// address in PUC (see `strcache`).
    pub(crate) fn c_chain_key(&self) -> u32 {
        // FNV-1a over 32-bit words
        let mut h = 0x811c_9dc5_u32 ^ self.g.nccalls;
        let mut mix = |x: u32| h = (h ^ x).wrapping_mul(0x0100_0193);
        for f in self.frames.iter() {
            if let CallFrame::Cont(c) = f {
                mix(match c.kind {
                    ContKind::Pcall { .. } => 1,
                    ContKind::Xpcall { .. } => 2,
                    ContKind::Meta(_) => 3,
                    ContKind::Pairs { .. } => 4,
                    ContKind::Close(_) => 5,
                    ContKind::Host(_) => 6,
                });
            }
        }
        for n in self.running_natives.iter() {
            mix(n.nc.f as usize as u32);
        }
        h
    }
}
