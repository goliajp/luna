//! The numeric and generic `for`, laid out as each dialect's parser lays
//! them out (`fornum`, `forlist`, `forbody`): the hidden control values,
//! then the loop variables, in the registers PUC gives them.

use super::*;
use crate::vm::isa::ForLayout;

impl<'a> Compiler<'a> {
    /// Whether a block's variables leave scope before the `CLOSE` that ends
    /// it (PUC 5.1 / 5.4 `leaveblock` remove them first; 5.2 / 5.3 / 5.5
    /// close first).
    pub(super) fn vars_end_before_close(&self) -> bool {
        matches!(self.version, LuaVersion::Lua51 | LuaVersion::Lua54)
    }

    /// The hidden control values of a loop, as locals `debug.getlocal`
    /// lists, live from here to the end of the loop's block.
    fn declare_hidden(&mut self, base: u32, names: &[&'a str]) -> Result<(), SyntaxError> {
        for (i, &name) in names.iter().enumerate() {
            self.declare_local(name, base + i as u32, false)?;
        }
        Ok(())
    }

    /// The body in a block of its own (PUC `block`), then the end of the
    /// loop variables' scope (PUC's `leaveblock` in `forbody`): a `CLOSE`
    /// when one of them, from local `first` on, is captured.
    fn for_body(&mut self, body: &Block, first: usize, floor: u32) -> Result<(), SyntaxError> {
        self.enter_block(false);
        self.stat_block(body)?;
        self.leave_block()?;
        let captured = self.lr().locals[first..].iter().any(|l| l.captured);
        let before = self.here() as u32;
        if captured {
            self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
        }
        let end = if self.vars_end_before_close() {
            before
        } else {
            self.here() as u32
        };
        let b = self.l().blocks.last_mut().expect("loop block");
        b.body_end = Some((first, end));
        b.for_loop = true;
        Ok(())
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
        let layout = match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 => ForLayout::Gen53,
            LuaVersion::Lua55 => ForLayout::Gen55,
            _ => ForLayout::Gen54,
        };
        let (prep_op, call_op, loop_op) = layout.ops();
        // PUC `forlist`: the expression list fills the iterator, state and
        // control (5.4+: and the closing value), then `luaK_checkstack`
        // leaves room to call the iterator past them
        let values = if layout == ForLayout::Gen53 { 3 } else { 4 };
        let base = self.explist_adjust(exprs, values)?;
        self.set_freereg(base + values);
        self.reserve(layout.call_end() - values)?;
        self.set_freereg(base + values);
        self.enter_block(true);
        let hidden: &[&'a str] = match layout {
            ForLayout::Gen53 => &["(for generator)", "(for state)", "(for control)"],
            ForLayout::Gen54 => &["(for state)"; 4],
            _ => &["(for state)"; 3],
        };
        self.declare_hidden(base, hidden)?;
        // the closing value: a `return f()` in the body is no tail call,
        // and a `goto` leaving the loop closes it
        // PUC enters the loop's block before the expressions: its level is
        // the base
        let b = self.l().blocks.last_mut().expect("no block");
        b.reg_floor = base;
        if layout.closing().is_some() {
            b.tbc_scope = true;
            b.has_tbc = true;
        }
        let vbase = base + layout.var();
        self.set_freereg(vbase);
        let prep = self.emit(Inst::iabx(prep_op, base, 0));
        let body_first = self.lr().locals.len();
        let nvars = vars.len() as u32;
        for (i, v) in vars.iter().enumerate() {
            // 5.5: the control (first) variable is read-only
            self.declare_local(
                self.nm(v),
                vbase + i as u32,
                i == 0 && self.version >= LuaVersion::Lua55,
            )?;
        }
        self.reserve(nvars)?;
        let body_top = self.here();
        self.for_body(body, body_first, vbase)?;
        let tforcall_pc = self.here();
        let skip = tforcall_pc - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(prep_op, base, skip as u32);
        self.mark_target(tforcall_pc);
        // 5.1's back `JMP` (luna's `TForLoop53`) takes the line of the
        // body's last token, as PUC emits it after `luaK_fixline`
        let body_last = self.last_line;
        // PUC `forbody` fixes TFORCALL/TFORLOOP to the line of the first token
        // after `in` (the EXPR's source line). A non-callable iterator
        // (`for k,v in 3 do ...`) then raises on the EXPR's line, not the
        // `for` line (errors.lua :428/:429).
        self.last_line = expr_line;
        self.emit(Inst::iabc(call_op, base, 0, nvars, false));
        let back = self.here() - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        if self.version == LuaVersion::Lua51 {
            self.last_line = body_last;
        }
        self.emit(Inst::iabx(loop_op, base, back as u32));
        self.mark_target(body_top);
        // the block's CLOSE of the closing value (PUC emits it in
        // `leaveblock` after reading the loop's `end`, so a line hook sees
        // that line once as the loop exits)
        if layout.closing().is_some()
            && let Some(line) = end_line
        {
            self.last_line = line;
        }
        self.l().blocks.last_mut().expect("for block").end_line = end_line;
        self.leave_block()?;
        self.set_freereg(base);
        if let Some(line) = end_line {
            self.last_line = line;
        }
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
        // 5.5 keeps the index in the loop variable: its `forprep` drops one
        // of the three registers the values took
        let v55 = self.version >= LuaVersion::Lua55;
        let layout = if v55 {
            ForLayout::Num55
        } else {
            ForLayout::Num
        };
        let (prep_op, _, loop_op) = layout.ops();
        self.set_freereg(base + 3);
        self.enter_block(true);
        self.l().blocks.last_mut().expect("no block").reg_floor = base;
        let hidden: &[&'a str] = match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 => {
                &["(for index)", "(for limit)", "(for step)"]
            }
            LuaVersion::Lua55 => &["(for state)"; 2],
            _ => &["(for state)"; 3],
        };
        self.declare_hidden(base, hidden)?;
        self.last_line = line;
        let var_reg = base + layout.var();
        self.set_freereg(var_reg);
        let prep = self.emit(Inst::iabx(prep_op, base, 0));
        let body_first = self.lr().locals.len();
        self.declare_local(var, var_reg, v55)?;
        self.reserve(1)?;
        let body_top = self.here();
        self.for_body(body, body_first, var_reg)?;
        let loop_pc = self.here();
        let back = loop_pc - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        // PUC attributes FORLOOP (the per-iteration back-edge) to the `for` line,
        // so each loop iteration re-fires a line event there.
        self.last_line = line;
        self.emit(Inst::iabx(loop_op, base, back as u32));
        let skip = self.here() - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(prep_op, base, skip as u32);
        // ForLoop's back-edge lands at `body_top`; ForPrep's forward-skip
        // lands at the post-loop pc (= `here()` after the ForLoop emit).
        self.mark_target(body_top);
        let post_loop = self.here();
        self.mark_target(post_loop);
        self.l().blocks.last_mut().expect("for block").end_line = end_line;
        self.leave_block()?;
        self.set_freereg(base);
        if let Some(line) = end_line {
            self.last_line = line;
        }
        Ok(())
    }
}
