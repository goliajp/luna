//! The numeric and generic `for`.

use super::*;

impl<'a> Compiler<'a> {
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
        // PUC `forlist`'s `luaK_checkstack`: room to call the generator
        // past the control slots, to `base + 7` in 5.4 and `base + 6`
        // otherwise
        let room = if self.version == LuaVersion::Lua54 {
            3
        } else {
            2
        };
        self.reserve(room)?;
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
            self.l().locvars.push_or_abort(crate::runtime::LocVar {
                name: name.into(),
                reg: base + off,
                start_pc,
                end_pc,
            });
        }
    }
}
