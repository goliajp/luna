//! `if`, `while`, `repeat` and both `for` loops.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn if_stat(
        &mut self,
        arms: &[ast::IfArm],
        else_body: Option<&Block>,
    ) -> Result<(), SyntaxError> {
        let mut end_jumps = Jumps::new();
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

    pub(super) fn numeric_for(
        &mut self,
        var: &'a str,
        line: u32,
        (start, limit, step): (ExprId, ExprId, Option<ExprId>),
        body: &Block,
        end_line: Option<u32>,
    ) -> Result<(), SyntaxError> {
        self.last_line = line;
        let base = self.lr().freereg;
        self.set_freereg(base);
        let se = self.expr(start)?;
        self.set_freereg(base);
        let s0 = self.exp_to_nextreg(se)?;
        debug_assert_eq!(s0, base);
        let le = self.expr(limit)?;
        self.set_freereg(base + 1);
        let l0 = self.exp_to_nextreg(le)?;
        debug_assert_eq!(l0, base + 1);
        match step {
            Some(st) => {
                let ste = self.expr(st)?;
                self.set_freereg(base + 2);
                let st0 = self.exp_to_nextreg(ste)?;
                debug_assert_eq!(st0, base + 2);
            }
            None => {
                self.set_freereg(base + 2);
                self.reserve(1)?;
                self.emit(Inst::iasbx(Op::LoadI, base + 2, 1));
            }
        }
        self.set_freereg(base + 3);
        let control_start = self.here() as u32;
        self.enter_block(true);
        let var_reg = self.reserve(1)?;
        self.declare_local(var, var_reg, self.version >= LuaVersion::Lua55)?;
        let body_first = self.lr().locals.len();
        self.last_line = line;
        let prep = self.emit(Inst::iabx(Op::ForPrep, base, 0));
        let body_top = self.here();
        self.stat_block(body)?;
        if self.block_captured() {
            self.close_body(body_first, var_reg);
        }
        let loop_pc = self.here();
        let back = loop_pc - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        // PUC attributes FORLOOP (the per-iteration back-edge) to the `for` line,
        // so each loop iteration re-fires a line event there.
        self.last_line = line;
        self.emit(Inst::iabx(Op::ForLoop, base, back as u32));
        let skip = self.here() - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(Op::ForPrep, base, skip as u32);
        // ForLoop's back-edge lands at `body_top`; ForPrep's forward-skip
        // lands at the post-loop pc (= `here()` after the ForLoop emit).
        self.mark_target(body_top);
        let post_loop = self.here();
        self.mark_target(post_loop);
        self.l().blocks.last_mut().expect("for block").end_line = end_line;
        self.leave_block()?;
        // PUC fornum's internal locals, which debug.getlocal lists ahead
        // of the loop variable: 5.1-5.3 name them after their roles, 5.4
        // has three "(for state)", 5.5 two.
        let hidden: &[(&str, u32)] = match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 => {
                &[("(for index)", 0), ("(for limit)", 1), ("(for step)", 2)]
            }
            LuaVersion::Lua55 => &[("(for state)", 0), ("(for state)", 1)],
            _ => &[("(for state)", 0), ("(for state)", 1), ("(for state)", 2)],
        };
        self.push_hidden_locals(base, hidden, control_start, post_loop as u32);
        self.set_freereg(base);
        Ok(())
    }

    /// Debug entries for a for loop's internal variables, `(name, offset
    /// from base)`, live over `start_pc..end_pc`.
    pub(super) fn push_hidden_locals(
        &mut self,
        base: u32,
        hidden: &[(&str, u32)],
        start_pc: u32,
        end_pc: u32,
    ) {
        for &(name, off) in hidden {
            self.l().locvars.push(crate::runtime::LocVar {
                name: name.into(),
                reg: base + off,
                start_pc,
                end_pc,
            });
        }
    }

    pub(super) fn generic_for(
        &mut self,
        vars: &'a [Name],
        exprs: &[ExprId],
        body: &Block,
        expr_line: u32,
        end_line: Option<u32>,
    ) -> Result<(), SyntaxError> {
        let line = vars[0].line;
        self.last_line = line;
        // control slots: iterator, state, control, closing (<close>: slice 5).
        // Before 5.4 the list is cut to three values (PUC `forlist`'s
        // `adjust_assign(ls, 3, ...)`) and the fourth slot stays nil, so a
        // fourth value is evaluated and dropped rather than closed.
        let tbc = self.version >= LuaVersion::Lua54;
        let base = if tbc {
            self.explist_adjust(exprs, 4)?
        } else {
            let base = self.explist_adjust(exprs, 3)?;
            self.set_freereg(base + 3);
            self.reserve(1)?;
            self.emit(Inst::iabc(Op::LoadNil, base + 3, 0, 0, false));
            base
        };
        self.set_freereg(base + 4);
        let control_start = self.here() as u32;
        self.enter_block(true);
        // the 4th control value is an implicit to-be-closed variable (5.4+);
        // a `return f()` in the body must not be a tail call, *and* a `goto`
        // leaving this block must close the iterator's closing value via a
        // trampoline (locals.lua:1219 nested-for goto regression).
        if tbc {
            let b = self.l().blocks.last_mut().expect("no block");
            b.tbc_scope = true;
            b.has_tbc = true;
        }
        let nvars = vars.len() as u32;
        let vbase = self.reserve(nvars)?;
        debug_assert_eq!(vbase, base + 4);
        for (i, v) in vars.iter().enumerate() {
            // 5.5: the control (first) variable is read-only
            self.declare_local(
                self.nm(v),
                vbase + i as u32,
                i == 0 && self.version >= LuaVersion::Lua55,
            )?;
        }
        let body_first = self.lr().locals.len();
        let prep = self.emit(Inst::iabx(Op::TForPrep, base, 0));
        let body_top = self.here();
        self.stat_block(body)?;
        if self.block_captured() {
            self.close_body(body_first, vbase);
        }
        let tforcall_pc = self.here();
        let skip = tforcall_pc - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(Op::TForPrep, base, skip as u32);
        // TForPrep's forward-skip lands at `tforcall_pc` (the upcoming TForCall
        // emit position). Mark before the TForCall emit advances `here()`.
        self.mark_target(tforcall_pc);
        // PUC `forbody` fixes TFORCALL/TFORLOOP to the line of the first token
        // after `in` (the EXPR's source line). A non-callable iterator
        // (`for k,v in 3 do ...`) then raises on the EXPR's line, not the
        // `for` line (errors.lua :428/:429).
        self.last_line = expr_line;
        self.emit(Inst::iabc(Op::TForCall, base, 0, nvars, false));
        let back = self.here() - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.emit(Inst::iabx(Op::TForLoop, base, back as u32));
        // TForLoop's back-edge lands at `body_top` (per-iteration restart).
        self.mark_target(body_top);
        // Override the body block's reg_floor to `base` so trampoline OP_Close
        // emitted for a `goto` leaving the loop closes the iterator's closing
        // value at `base + 3` (which sits BELOW the for-body's user-locals
        // floor `base + 4`). PUC's lparser does the same via `leavelevel` to
        // `f->level + 4` minus the to-be-closed control width.
        let blk = self.l().blocks.last_mut().expect("no block");
        blk.reg_floor = base;
        blk.end_line = end_line;
        self.leave_block()?;
        // close the iterator's closing value (4th control slot, 5.4+). PUC
        // emits it in `leaveblock` after reading the loop's `end`, so a line
        // hook sees that line once as the loop exits.
        if self.version >= LuaVersion::Lua54
            && let Some(line) = end_line
        {
            self.last_line = line;
        }
        self.emit(Inst::iabc(Op::Close, base, 0, 0, false));
        // PUC forlist registers hidden control variables that
        // debug.getlocal lists; they live across the loop body. 5.1-5.3
        // have three, named after their roles. 5.4 names all four control
        // slots "(for state)" — generator, state, control, and
        // to-be-closed; 5.5 dropped the user-control entry so only three
        // are reported. 5.4 files.lua :443 expects the to-be-closed at the
        // 4th "(for state)" hit; 5.5 files.lua :433 expects it at the 3rd.
        // Without the user-control entry on 5.4 the file never gets closed
        // on `break`.
        let end_pc = self.here() as u32;
        let hidden: &[(&str, u32)] = match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 => &[
                ("(for generator)", 0),
                ("(for state)", 1),
                ("(for control)", 2),
            ],
            LuaVersion::Lua55 => &[("(for state)", 0), ("(for state)", 1), ("(for state)", 3)],
            _ => &[
                ("(for state)", 0),
                ("(for state)", 1),
                ("(for state)", 2),
                ("(for state)", 3),
            ],
        };
        self.push_hidden_locals(base, hidden, control_start, end_pc);
        self.set_freereg(base);
        Ok(())
    }
}
