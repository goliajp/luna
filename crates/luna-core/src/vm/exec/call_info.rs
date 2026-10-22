//! PUC 5.2+'s `CallInfo`s: one per call level, allocated the first time a
//! thread reaches the level (`luaE_extendCI`) and kept until the collector
//! frees the unused ones. Only the count is kept, for what they take of
//! `totalbytes`.

use super::*;

impl Vm {
    /// The `CallInfo`s the running thread uses besides its base one: its
    /// frames as 5.1 counts them in its array, less a coroutine's base
    /// `CallInfo` (the main thread counts no base: the host's frames are
    /// below its own) and the continuations of the host's `lua_pcall`s.
    #[inline(always)]
    pub(super) fn ci_in_use(&self) -> u32 {
        let base = u32::from(self.current.is_some());
        self.frames_in_use() - base - self.g.host_conts
    }

    /// A Lua function's call level was entered: counted when the thread
    /// has not been this deep before. `ci_alloc` stays at its maximum in
    /// 5.1, which counts its array where it grows (`grow_frames`). Only
    /// the frame count is compared here, against the count below which a
    /// push needs no `CallInfo` (`CallGuards::ci_hint`); the fast loop's
    /// calls test it before they push.
    #[inline(always)]
    pub(super) fn note_call_level(&mut self) {
        if self.frames.len() as u32 > self.g.ci_hint {
            self.note_level_out_of_line();
        }
        #[cfg(any(debug_assertions, feature = "gc-verify"))]
        assert!(
            self.ci_in_use() <= self.g.ci_alloc,
            "a call level went uncounted: the frame count bound is stale"
        );
    }

    #[inline(never)]
    fn note_level_out_of_line(&mut self) {
        self.note_level();
    }

    /// A native was entered: one level more, so the frame count bound one
    /// less (given back when it returns, see `finish_native_call`).
    #[inline(always)]
    pub(crate) fn note_native_level(&mut self) {
        if self.frames.len() as u32 >= self.g.ci_hint {
            self.note_level();
        } else {
            self.g.ci_hint -= 1;
        }
        #[cfg(any(debug_assertions, feature = "gc-verify"))]
        assert!(
            self.ci_in_use() <= self.g.ci_alloc,
            "a native's level went uncounted: the frame count bound is stale"
        );
    }

    /// A call level was entered, of a native or of a compiled call (whose
    /// counts are outside the frames): counted as for a Lua function, and
    /// the frame count up to which no more is needed worked out again.
    #[inline]
    pub(crate) fn note_level(&mut self) {
        let n = self.ci_in_use();
        if n > self.g.ci_alloc {
            self.extend_ci_to(n);
        }
        let room = self.frames.capacity() as u32;
        // 5.1 counts no `CallInfo` here (`ci_alloc` stays at its maximum):
        // only the room bounds the frames then
        let free = (self.frames.len() as u32).checked_add(self.g.ci_alloc - n);
        self.g.ci_hint = free.map_or(room, |h| h.min(room));
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
