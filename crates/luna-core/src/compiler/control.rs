//! `if`, `while` and `repeat`, as each dialect's parser emits them
//! (`ifstat`, `test_then_block`, `whilestat`, `repeatstat`).

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn if_stat(
        &mut self,
        arms: &'a [ast::IfArm],
        else_body: Option<&'a Block>,
    ) -> Result<(), SyntaxError> {
        if self.version == LuaVersion::Lua51 {
            return self.if_stat_51(arms, else_body);
        }
        let mut escape = NO_JUMP;
        for (i, arm) in arms.iter().enumerate() {
            let more = i + 1 < arms.len() || else_body.is_some();
            self.test_then_block(arm, more, &mut escape)?;
        }
        if let Some(b) = else_body {
            self.block_scoped(b)?;
        }
        self.patch_to_here(escape)
    }

    /// 5.1 `ifstat`: each arm's false jumps go to the next arm.
    fn if_stat_51(
        &mut self,
        arms: &'a [ast::IfArm],
        else_body: Option<&'a Block>,
    ) -> Result<(), SyntaxError> {
        let mut escape = NO_JUMP;
        let mut flist = NO_JUMP;
        for (i, arm) in arms.iter().enumerate() {
            if i > 0 {
                let j = self.jump()?;
                self.concat_list(&mut escape, j)?;
                self.patch_to_here(flist)?;
            }
            flist = self.cond(arm.cond)?;
            self.block_scoped(&arm.body)?;
        }
        match else_body {
            Some(b) => {
                let j = self.jump()?;
                self.concat_list(&mut escape, j)?;
                self.patch_to_here(flist)?;
                self.block_scoped(b)?;
            }
            None => self.concat_list(&mut escape, flist)?,
        }
        self.patch_to_here(escape)
    }

    /// 5.2+ `test_then_block`; `more`: an `elseif` or `else` follows.
    fn test_then_block(
        &mut self,
        arm: &'a ast::IfArm,
        more: bool,
        escape: &mut i32,
    ) -> Result<(), SyntaxError> {
        let e = self.expr(arm.cond)?;
        let stats = self.ls(arm.body.stats);
        // 5.2–5.4 test the condition once `then` is read; 5.5 before
        if self.version <= LuaVersion::Lua54 {
            self.last_line = arm.then_line;
        }
        let jumps_out = match stats.first().map(|&s| self.ast.stat(s)) {
            Some(Stat::Break { line }) if self.version <= LuaVersion::Lua54 => {
                Some(("break", *line))
            }
            Some(Stat::Goto(n)) if self.version <= LuaVersion::Lua53 => Some((self.nm(n), n.line)),
            _ => None,
        };
        let jf = match jumps_out {
            Some((target, line)) => {
                // the condition's true jumps are the `break` / `goto`
                let e = self.go_if_false(e)?;
                let (_, t, _) = self.exp_parts(e);
                self.enter_block(false);
                self.last_line = line;
                self.cond_break(t, target)?;
                if stats.len() == 1 {
                    return self.leave_block();
                }
                let jf = self.jump()?;
                self.stat_list(&stats[1..], false)?;
                jf
            }
            None => {
                let e = if self.version >= LuaVersion::Lua55 {
                    self.cond_of(e)?
                } else {
                    self.go_if_true(e)?
                };
                let (_, _, f) = self.exp_parts(e);
                self.enter_block(false);
                self.stat_block(&arm.body)?;
                f
            }
        };
        self.leave_block()?;
        if more {
            let j = self.jump()?;
            self.concat_list(escape, j)?;
        }
        self.patch_to_here(jf)
    }

    pub(super) fn while_stat(
        &mut self,
        cond: ExprId,
        body: &'a Block,
        end_line: Option<u32>,
    ) -> Result<(), SyntaxError> {
        let init = self.get_label();
        let exit = self.cond(cond)?;
        self.enter_block(true);
        self.block_scoped(body)?;
        let j = self.jump()?;
        self.patch_list(j, init)?;
        if let Some(line) = end_line {
            self.last_line = line;
        }
        self.leave_block()?;
        self.patch_to_here(exit)
    }

    pub(super) fn repeat_stat(&mut self, body: &'a Block, cond: ExprId) -> Result<(), SyntaxError> {
        let init = self.get_label();
        self.enter_block(true);
        self.enter_block(false);
        self.stat_block_inner(body, true)?;
        let mut exit = self.cond(cond)?;
        let upval = self.block_captured();
        let level = self.block_floor();
        match self.version {
            LuaVersion::Lua51 if upval => {
                // `if cond then break end`, closing, then repeat
                self.break_stat(self.last_line)?;
                self.patch_to_here(exit)?;
                self.leave_block()?;
                let j = self.jump()?;
                self.patch_list(j, init)?;
            }
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 => {
                if upval {
                    self.patch_close(exit, level);
                }
                self.leave_block()?;
                self.patch_list(exit, init)?;
            }
            _ => {
                if self.version == LuaVersion::Lua54 {
                    self.leave_block()?;
                }
                if upval {
                    let out = self.jump()?;
                    self.patch_to_here(exit)?;
                    self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                    exit = self.jump()?;
                    self.patch_to_here(out)?;
                }
                self.patch_list(exit, init)?;
                if self.version != LuaVersion::Lua54 {
                    self.leave_block()?;
                }
            }
        }
        self.leave_block()
    }

    /// `break`.
    pub(super) fn break_stat(&mut self, line: u32) -> Result<(), SyntaxError> {
        self.jump_line(line);
        match self.version {
            LuaVersion::Lua51 => {
                // the blocks left up to the loop: a CLOSE when one of them
                // has a captured local
                let mut upval = false;
                let mut li = None;
                for (i, b) in self.lr().blocks.iter().enumerate().rev() {
                    if b.is_loop != 0 {
                        li = Some(i);
                        break;
                    }
                    upval |= self.lr().locals[b.first_local..].iter().any(|l| l.captured);
                }
                let li = li.expect("the frontend checks break");
                if upval {
                    let level = self.reg_level(self.lr().blocks[li].first_avar);
                    self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                }
                let j = self.jump()?;
                let mut list = self.lr().blocks[li].breaklist;
                self.concat_list(&mut list, j)?;
                self.l().blocks[li].breaklist = list;
                Ok(())
            }
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let j = self.jump()?;
                self.goto_list_53("break", j)
            }
            LuaVersion::Lua54 => {
                let j = self.jump()?;
                self.new_goto("break", j);
                Ok(())
            }
            _ => {
                let b = self.l().blocks.iter_mut().rev().find(|b| b.is_loop != 0);
                b.expect("the frontend checks break").is_loop = 2;
                self.goto_55("break")?;
                Ok(())
            }
        }
    }
}
