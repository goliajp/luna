//! `if`, `while`, `repeat` and both `for` loops.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn if_stat(
        &mut self,
        arms: &[ast::IfArm],
        else_body: Option<&Block>,
    ) -> Result<(), SyntaxError> {
        let mut end_jumps = Jumps::new(self.heap.mem());
        for (
            i,
            ast::IfArm {
                cond,
                then_line,
                body,
            },
        ) in arms.iter().enumerate()
        {
            let (skips, last) = self.cond_jump_false(*cond)?;
            // PUC 5.2/5.3/5.4 attribute BOTH the TEST and the conditional-skip
            // JMP to the `then` keyword's line, because `luaK_goiftrue`
            // emits them after `checknext(TK_THEN)` has advanced
            // `ls->lastline` past the keyword. The result is that a taken
            // if-arm fires a line-hook event for the `then` line between
            // the condition's last instruction and the body's first
            // (5.2/5.3/5.4 db.lua first `test` baselines {2,3,4,7}). PUC
            // 5.5 reorders luaK_goiftrue so the test/jmp keep the condition
            // line (5.5 db.lua expects {2,4,7}). Only a `TEST` of the
            // condition's last operand is emitted there: a comparison was
            // emitted where it was read, and the left operand of an `and` /
            // `or` was tested at its operator.
            if self.version >= LuaVersion::Lua52
                && self.version <= LuaVersion::Lua54
                && last == cond::LastTest::Test
            {
                let jmp = self.here() - 1;
                self.l().lines[jmp] = *then_line;
                self.l().lines[jmp - 1] = *then_line;
            }
            self.block_scoped(body)?;
            let is_last = i == arms.len() - 1 && else_body.is_none();
            if !is_last {
                end_jumps.push(self.emit_jump());
            }
            for skip in skips.iter() {
                self.patch_to_here(skip)?;
            }
        }
        if let Some(eb) = else_body {
            self.block_scoped(eb)?;
        }
        for j in end_jumps.iter() {
            self.patch_to_here(j)?;
        }
        Ok(())
    }

    /// A loop's per-iteration CLOSE of its body (from local `first` on). 5.4
    /// ends the body's scope before it (see [`Compiler::leave_block`]).
    pub(super) fn close_body(&mut self, first: usize, floor: u32) {
        if self.version == LuaVersion::Lua54 {
            let here = self.here() as u32;
            self.l().blocks.last_mut().expect("loop block").body_end = Some((first, here));
        }
        self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
    }

    pub(super) fn while_stat(
        &mut self,
        cond: ExprId,
        body: &Block,
        end_line: Option<u32>,
    ) -> Result<(), SyntaxError> {
        let top = self.here();
        let (exits, _) = self.cond_jump_false(cond)?;
        self.enter_block(true);
        self.stat_block(body)?;
        if self.block_captured() {
            let floor = self.block_floor();
            let first = self.l().blocks.last().expect("while block").first_local;
            self.close_body(first, floor);
        }
        self.jump_back(top)?;
        self.l().blocks.last_mut().expect("while block").end_line = end_line;
        self.leave_block()?;
        for exit in exits.iter() {
            self.patch_to_here(exit)?;
        }
        Ok(())
    }

    pub(super) fn repeat_stat(&mut self, body: &Block, cond: ExprId) -> Result<(), SyntaxError> {
        let top = self.here();
        self.enter_block(true);
        self.stat_block_inner(body, true)?;
        // the condition's jumps are taken when it is false (loop again) and
        // the code falls through when it is true (exit), as for `while`. With
        // no captured body local they go straight back. When a body local is
        // captured, the loop-back path must first CLOSE its upvalues and the
        // normal exit must jump over that close-and-loop tail (PUC
        // `repeatstat`).
        let (again, _) = self.cond_jump_false(cond)?;
        if self.block_captured() {
            let floor = self.block_floor();
            let exit = self.emit_jump();
            for pc in again.iter() {
                self.patch_to_here(pc)?;
            }
            let first = self.l().blocks.last().expect("repeat block").first_local;
            self.close_body(first, floor);
            self.jump_back(top)?;
            self.patch_to_here(exit)?;
        } else {
            for pc in again.iter() {
                self.patch_back(pc, top)?;
            }
        }
        self.leave_block()?;
        Ok(())
    }
}
