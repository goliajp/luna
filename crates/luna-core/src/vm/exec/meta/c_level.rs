//! The C call level count (PUC `nCcalls`) that metamethod and native calls take.

use super::*;

impl Vm {
    /// The level a call made while a message handler runs fails at with
    /// "error in error handling" (see `ERRERR_C_DEPTH`).
    pub(crate) fn errerr_c_depth(&self) -> u32 {
        if self.version >= LuaVersion::Lua54 {
            ERRERR_C_DEPTH
        } else {
            ERRERR_C_DEPTH_PRE54
        }
    }

    /// PUC's check before a call that takes a C level (5.4
    /// `luaE_checkcstack`, 5.1 `luaD_call`): the call that would run at
    /// `MAX_C_DEPTH` fails with "C stack overflow", positioned when Lua
    /// code made it; a message handler running on that error gets a tenth
    /// more levels before "error in error handling". Whoever passes the
    /// check takes the level (`c_depth` or `pcall_depth`).
    #[inline(always)]
    pub(crate) fn check_c_level(&mut self, positioned: bool) -> Result<(), LuaError> {
        if self.g.nccalls + 1 < MAX_C_DEPTH {
            return Ok(());
        }
        self.c_level_overflow(positioned)
    }

    /// [`Vm::check_c_level`] at the limit.
    #[cold]
    #[inline(never)]
    fn c_level_overflow(&mut self, positioned: bool) -> Result<(), LuaError> {
        let next = self.g.nccalls + 1;
        if next == MAX_C_DEPTH || self.msgh_depth == 0 {
            let e = if positioned {
                self.runerror("C stack overflow")
            } else {
                self.runerror_in_c("C stack overflow")
            };
            // the refused call keeps its level while its handler runs
            self.c_overflow_err = Some(e.0);
            return Err(e);
        }
        if next >= self.errerr_c_depth() {
            return Err(LuaError(self.errerr()));
        }
        Ok(())
    }

    /// Count a metamethod, `__pairs` or `__close` call as PUC counts the C
    /// call it makes. The caller pushes the continuation that holds the
    /// level.
    pub(in crate::vm::exec) fn enter_c_level(&mut self, positioned: bool) -> Result<(), LuaError> {
        self.check_c_level(positioned)?;
        self.take_c_level();
        Ok(())
    }

    /// Take the level [`Vm::check_c_level`] allowed, for a continuation
    /// pushed that gives it back. Read only by 5.1, the count of these
    /// continuations is kept by every dialect: an increment costs less than
    /// the test that would skip it.
    #[inline(always)]
    pub(in crate::vm::exec) fn take_c_level(&mut self) {
        self.g.nccalls += 1;
        self.g.meta_conts += 1;
    }

    /// PUC `lua_resume` at the C level limit. It refuses a resumer whose
    /// count has reached the limit (5.1, 5.4 on) or would with the resume
    /// (5.2, 5.3): the thread keeps its state, except in 5.1, whose refusal
    /// drops the thread's function so that the thread reads as dead. From
    /// 5.4 a resume one level below the limit goes in, and the call that
    /// starts or continues the body fails there: the thread dies with the
    /// error.
    pub(in crate::vm::exec) fn resume_c_level(&mut self, co: Gc<Coro>) -> Result<(), LuaError> {
        let n = self.g.nccalls;
        let v = self.version;
        let low = native_stack::is_low(native_stack::RESERVE);
        let refused = match v {
            LuaVersion::Lua52 | LuaVersion::Lua53 => n + 1 >= MAX_C_DEPTH,
            _ => n >= MAX_C_DEPTH,
        };
        if !refused && !low && !(v >= LuaVersion::Lua54 && n + 1 >= MAX_C_DEPTH) {
            return Ok(());
        }
        // `lua_resume`'s own refusal is `resume_error`, which checks for
        // nothing; past it, `ccall`'s `luaE_checkcstack` raises
        let e = if refused || low {
            self.plain_err("C stack overflow")
        } else {
            self.runerror_in_c("C stack overflow")
        };
        if v == LuaVersion::Lua51 || (v >= LuaVersion::Lua54 && !refused && !low) {
            self.set_coro_status(co, CoroStatus::Dead);
            if v >= LuaVersion::Lua54 {
                self.set_coro_error(co, e.0, crate::runtime::ErrorStatus::Run);
            }
            self.host_thread_reset(co);
        }
        Err(e)
    }
}
