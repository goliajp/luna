//! Count and line hooks fired between instructions.

use super::*;

impl Vm {
    /// Count and line hooks (PUC `traceexec`), fired before the instruction
    /// at `pc` of `cl` runs; `oldpc` is where the frame's line hook last looked.
    #[inline(never)]
    pub(super) fn exec_hooks(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        oldpc: u32,
    ) -> Result<(), LuaError> {
        let lines = &cl.proto.lines;
        // a 5.1 generic `for` is PUC's `TFORLOOP` and the `JMP` after it,
        // which `TFORLOOP` takes itself: that jump (luna's `TForLoop53`)
        // is no instruction a hook sees
        if self.version == LuaVersion::Lua51
            && cl.proto.code.get(pc as usize).map(|i| i.op()) == Some(Op::TForLoop53)
        {
            return Ok(());
        }
        // count hook: fire every `count_base` instructions
        let mut counthook = false;
        if self.hook.count {
            self.hook.count_left -= 1;
            if self.hook.count_left <= 0 {
                self.hook.count_left = self.hook.count_base;
                counthook = true;
            }
        }
        // the instruction a hook yielded at: its hooks have run (5.2+)
        if self.hook_resumed && (counthook || self.hook.line) {
            self.hook_resumed = false;
            return Ok(());
        }
        if counthook {
            // hooked function is the running Lua frame: its frame
            // is on the stack, so no synthetic C level is needed.
            // a count event carries no line (PUC `luaD_hook(L,
            // LUA_HOOKCOUNT, -1)`): the Lua hook gets nil
            self.run_hook(b"count", None, false)?;
        }
        // line hook: fire on a fresh frame, a backward jump (loop), or a
        // change of source line.
        if self.hook.line {
            if lines.is_empty() {
                // PUC: a stripped chunk has no line info, so
                // `getfuncline` returns -1. The line hook still fires
                // on the first instruction of the new frame (where
                // `npci <= oldpc` holds at oldpc=0), with the line
                // pushed as `nil` instead of an integer (db.lua :1030
                // "hook called without debug info for 1st instruction").
                if oldpc == u32::MAX {
                    self.run_hook(b"line", None, false)?;
                    self.top_frame_mut().hook_oldpc = pc;
                }
            } else {
                let newline = lines[(pc as usize).min(lines.len() - 1)];
                // PUC `traceexec`: fire on frame entry (`oldpc == MAX`),
                // on a backward jump (`pc < oldpc` — strict; an equal pc
                // would re-fire the install-site after `oldpc = pc`),
                // or when the source line changes.
                let fire = oldpc == u32::MAX
                    || pc < oldpc
                    || newline != lines[(oldpc as usize).min(lines.len() - 1)];
                if fire {
                    self.run_hook(b"line", Some(newline as i64), false)?;
                }
                self.top_frame_mut().hook_oldpc = pc;
            }
        }
        if std::mem::take(&mut self.hook_yield) {
            return Err(self.yield_from_hook(pc, counthook));
        }
        Ok(())
    }

    /// A C line or count hook yielded before the instruction at `pc` ran
    /// (PUC `luaG_traceexec`'s `L->status == LUA_YIELD`): suspend the
    /// coroutine with no values; the resume runs the instruction. 5.2+
    /// mark the instruction so its hooks are not called again, and put the
    /// count back one short of the event; 5.1 just runs it again.
    fn yield_from_hook(&mut self, pc: u32, counthook: bool) -> LuaError {
        let v51 = self.version == LuaVersion::Lua51;
        if counthook && !v51 {
            self.hook.count_left = 1;
        }
        let f = self.top_frame_mut();
        f.pc = pc;
        if v51 {
            // PUC 5.1 leaves `savedpc` at the instruction, so the line hook
            // compares with the line of the one before it
            f.hook_oldpc = pc.checked_sub(1).unwrap_or(u32::MAX);
        }
        self.yielding = Some((Vec::new(), HOOK_YIELD_SLOT, 0));
        LuaError(Value::Nil)
    }
}
