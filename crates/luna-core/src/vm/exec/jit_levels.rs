//! The call levels compiled self-recursive code makes on the native stack,
//! counted as PUC's `luaD_precall` counts them: the stack the callee's
//! frame needs, then its `CallInfo` (5.1 growing its array), and 5.2's
//! collection check. Compiled code calls in here only when a level
//! crosses what the last visit allowed (`JitLimits`).

use super::*;

/// When compiled code next needs [`Vm::jit_self_level`]: a level whose
/// base is at `base` or above, or deeper than `depth`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JitLimits {
    /// the lowest base of a level that tests the stack
    pub base: u32,
    /// the deepest level that needs nothing done
    pub depth: u32,
}

impl Vm {
    /// Enter level 0 of a compiled call of `cl` made at `func_slot` with
    /// `nargs` arguments, as `push_frame` enters an interpreted one: the
    /// stack (5.3 and 5.4 checking for a collection first), 5.1's
    /// `CallInfo` array, the level, 5.2's collection check. The arguments
    /// are on the stack, the caller's frame below. Undone by
    /// [`Vm::jit_leave_levels`] with what this returns.
    pub(super) fn jit_enter_levels(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
    ) -> Result<(u32, u32, u32), LuaError> {
        let saved = (
            self.jit.levels_base,
            self.g.frames_native,
            self.jit.levels_slot,
        );
        let p = cl.proto;
        let (base, max) = (func_slot + 1, u32::from(p.max_stack));
        let gc = matches!(self.version, LuaVersion::Lua53 | LuaVersion::Lua54);
        self.check_lua_stack(base + nargs, max, gc)?;
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            self.grow_frames()?;
        }
        self.grow_stack((base + max) as usize)?;
        self.jit.levels_base = self.g.frames_native;
        self.jit.levels_slot = base;
        self.g.frames_native += 1;
        self.note_call_level();
        self.g.jit_top = base + max;
        self.precall_checkpoint(base + max);
        Ok(saved)
    }

    /// The compiled call [`Vm::jit_enter_levels`] entered has returned.
    pub(super) fn jit_leave_levels(&mut self, saved: (u32, u32, u32)) {
        (
            self.jit.levels_base,
            self.g.frames_native,
            self.jit.levels_slot,
        ) = saved;
        self.g.jit_top = 0;
    }

    /// The levels compiled code may enter from level `depth`, its frame at
    /// `base`, before it calls [`Vm::jit_self_level`] again.
    #[doc(hidden)]
    pub fn jit_limits(&self, cl: Gc<LuaClosure>, depth: u32) -> JitLimits {
        let p = cl.proto;
        let (nparams, max) = (u32::from(p.num_params), u32::from(p.max_stack));
        // a level whose arguments' top plus its frame reaches the stack's
        // end tests it (`check_lua_stack`)
        let base = self.g.stack_last.saturating_sub(max + nparams);
        // the level that grows 5.1's array or takes a new `CallInfo`
        let depth = if self.g.frame_size != u32::MAX {
            depth + self.g.frame_size.saturating_sub(self.frames_in_use())
        } else {
            depth + self.g.ci_alloc.saturating_sub(self.ci_in_use())
        };
        // 5.2 checks for a collection at every level
        let depth = if self.version == LuaVersion::Lua52 && self.step_due() {
            depth.min(self.jit_depth())
        } else {
            depth
        };
        JitLimits { base, depth }
    }

    /// The depth of the compiled level running now.
    fn jit_depth(&self) -> u32 {
        self.g.frames_native - self.jit.levels_base - 1
    }

    /// Level `depth` (1 for the first self call) of compiled `cl`, its
    /// frame at `base`, entered as PUC enters a call (with `tail`, the
    /// callee of a tail call PUC places there before it moves it down): the stack its frame
    /// needs, its `CallInfo`, 5.2's collection check; then the limits
    /// for the levels after it. With `bare` the levels hold no object the
    /// collector must see, and their stack slots are cleared before a
    /// check; otherwise no collection is checked here.
    #[doc(hidden)]
    pub fn jit_self_level(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        depth: u32,
        bare: bool,
        tail: bool,
    ) -> Result<JitLimits, LuaError> {
        let p = cl.proto;
        let (nparams, max) = (u32::from(p.num_params), u32::from(p.max_stack));
        let entry = self.jit.levels_base;
        // the levels below are entered; the deeper ones returned
        self.g.frames_native = entry + depth;
        self.g.jit_top = base + max;
        let gc = bare && matches!(self.version, LuaVersion::Lua53 | LuaVersion::Lua54);
        if bare {
            self.jit_clear_levels(base + nparams)?;
        }
        if let Err(e) = self.check_lua_stack(base + nparams, max, gc) {
            return Err(self.jit_positioned(cl, e));
        }
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            if let Err(e) = self.grow_frames() {
                return Err(self.jit_positioned(cl, e));
            }
        }
        // a tail call's callee takes the level of its caller, after
        // 5.1–5.3 placed it above first; 5.4 and 5.5 reuse the frame
        let ci = !tail || self.version <= LuaVersion::Lua53;
        if ci {
            self.g.frames_native += 1;
            self.note_call_level();
        }
        if bare {
            self.jit_clear_levels(base + max)?;
            self.precall_checkpoint(base + max);
        }
        if tail && ci {
            self.g.frames_native -= 1;
        }
        Ok(self.jit_limits(cl, if tail { depth - 1 } else { depth }))
    }

    /// The stack slots of the compiled levels, up to `top`, hold nothing
    /// for the collector: PUC's hold the numbers they compute. The
    /// function slot below level 0 keeps its closure.
    fn jit_clear_levels(&mut self, top: u32) -> Result<(), LuaError> {
        self.grow_stack(top as usize)?;
        let from = self.jit.levels_slot as usize;
        self.stack[from..top as usize].fill(Value::Nil);
        Ok(())
    }

    /// "stack overflow" raised for a level of compiled `cl` is positioned
    /// at its self call, as the interpreter positions it in the caller;
    /// any other error is left as it is.
    fn jit_positioned(&mut self, cl: Gc<LuaClosure>, e: LuaError) -> LuaError {
        let overflow = matches!(e.0, Value::Str(s) if s.as_bytes().ends_with(b"stack overflow"));
        if overflow {
            self.jit_depth_error(cl)
        } else {
            e
        }
    }
}
