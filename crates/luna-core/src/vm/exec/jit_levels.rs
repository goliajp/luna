//! The call levels compiled self-recursive code makes on the native stack,
//! counted as PUC's `luaD_precall` counts them: the stack the callee's
//! frame needs, then its `CallInfo` (5.1 growing its array), and 5.2's
//! collection check. Compiled code calls in here only when a level
//! crosses what the last visit allowed (`JitLimits`).

use super::*;

/// When compiled code next needs [`Vm::jit_self_level`]: a level whose
/// base is at `base` or above.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JitLimits {
    /// the lowest base of a level that may need something done
    pub base: u32,
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
    ) -> Result<Option<(u32, u32, u32)>, LuaError> {
        let saved = (
            self.jit.levels_base,
            self.g.frames_native,
            self.jit.levels_slot,
        );
        let p = cl.proto;
        let (base, max) = (func_slot + 1, u32::from(p.max_stack));
        if self.jit_level_checks(base + nargs, max) {
            return Ok(None);
        }
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
        self.precall_checkpoint(base + max)?;
        Ok(Some(saved))
    }

    /// Whether a call whose arguments end at `top`, its frame `max` slots,
    /// would find a collection step due where PUC's `luaD_precall` checks:
    /// 5.2 after its `CallInfo` and stack (bounded here by the stack
    /// doubled and a `CallInfo` more), 5.3 and 5.4 before growing the
    /// stack. The interpreter makes such a call.
    fn jit_level_checks(&self, top: u32, max: u32) -> bool {
        let grows = top + max >= self.g.stack_last;
        match self.version {
            LuaVersion::Lua52 => {
                let s = &self.heap.sizes;
                let stack = if grows {
                    (2 * self.stack_size + max) as usize * s.tvalue
                } else {
                    0
                };
                self.jit_due_after(stack + s.callinfo)
            }
            LuaVersion::Lua53 | LuaVersion::Lua54 => grows && self.jit_due_after(0),
            _ => false,
        }
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

    /// The levels compiled code may enter from the level at `base`
    /// before it calls [`Vm::jit_self_level`] again. One bound on the base
    /// covers both what can happen: a level whose arguments' top plus its
    /// frame reaches the stack's end tests the stack; the level that grows
    /// 5.1's array or takes a new `CallInfo` (and in 5.2 any level while a
    /// collection step could fall due) is a number of levels deeper, each
    /// at least the smallest self call's step above the one before.
    #[doc(hidden)]
    pub fn jit_limits(&self, cl: Gc<LuaClosure>, base: u32) -> JitLimits {
        let p = cl.proto;
        let (nparams, max) = (u32::from(p.num_params), u32::from(p.max_stack));
        let stack = self.g.stack_last.saturating_sub(max + nparams);
        let levels = if self.version == LuaVersion::Lua52 && self.jit_level_checks(0, 0) {
            0
        } else if self.g.frame_size != u32::MAX {
            self.g.frame_size.saturating_sub(self.frames_in_use())
        } else {
            self.g.ci_alloc.saturating_sub(self.ci_in_use())
        };
        let deeper = base.saturating_add((levels + 1).saturating_mul(self_call_steps(p).0));
        JitLimits {
            base: stack.min(deeper),
        }
    }

    /// Level `depth` (1 for the first self call) of compiled `cl`, its
    /// frame at `base`, entered as PUC enters a call (with `tail`, the
    /// callee of a tail call PUC places there before it moves it down):
    /// the stack its frame needs, its `CallInfo`, 5.2's collection check;
    /// then the limits for the levels after it. `None` when the check would
    /// find a step due: the interpreter makes the call instead, the level
    /// not counted here.
    #[doc(hidden)]
    pub fn jit_self_level(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        depth: u32,
        tail: bool,
    ) -> Result<Option<JitLimits>, LuaError> {
        let p = cl.proto;
        let (nparams, max) = (u32::from(p.num_params), u32::from(p.max_stack));
        let entry = self.jit.levels_base;
        // the levels below are entered; the deeper ones returned
        self.g.frames_native = entry + depth;
        self.g.jit_top = base + max;
        if self.jit_level_checks(base + nparams, max) {
            return Ok(None);
        }
        let gc = matches!(self.version, LuaVersion::Lua53 | LuaVersion::Lua54);
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
        self.precall_checkpoint(base + max)?;
        if tail && ci {
            self.g.frames_native -= 1;
        }
        // a tail call's level is its caller's, whose base is below the
        // callee's by a self call's step at most
        let base = if tail {
            base.saturating_sub(self_call_steps(p).1)
        } else {
            base
        };
        Ok(Some(self.jit_limits(cl, base)))
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

/// The smallest and the largest step from a level's base to its callee's
/// over `p`'s self calls (every call of compiled code is one): the call's
/// register plus one.
fn self_call_steps(p: Gc<crate::runtime::Proto>) -> (u32, u32) {
    use crate::vm::isa::Op;
    let (lo, hi) = p
        .code
        .iter()
        .filter(|i| matches!(i.op(), Op::Call | Op::TailCall))
        .map(|i| i.a() + 1)
        .fold((u32::MAX, 0), |(lo, hi), s| (lo.min(s), hi.max(s)));
    (lo.max(1), hi)
}
