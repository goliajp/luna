//! The numeric and generic `for`, laid out as each dialect's parser lays
//! them out (`fornum`, `forlist`, `forbody`): the hidden control values,
//! then the loop variables, in the registers PUC gives them.

use super::*;
use crate::runtime::mem::LVec;
use crate::vm::isa::ForLayout;

impl<'a> Compiler<'a> {
    /// The hidden control values of a loop, as locals `debug.getlocal`
    /// lists, live from here to the end of the loop's block.
    fn declare_hidden(&mut self, base: u32, names: &[&'a str]) -> Result<(), SyntaxError> {
        for (i, &name) in names.iter().enumerate() {
            self.declare_local(name, NameSrc::literal(name), base + i as u32, false)?;
        }
        Ok(())
    }

    /// PUC `forbody` after the prepare op: the loop variables `vars` in a
    /// block of their own from register `vbase` on, the body in a block
    /// inside it.
    fn for_body(
        &mut self,
        sid: StatId,
        vars: &[(&'a str, NameSrc, bool)],
        vbase: u32,
        body: &'a Block,
    ) -> Result<(), SyntaxError> {
        self.enter_block(false);
        for (i, &(name, src, read_only)) in vars.iter().enumerate() {
            self.declare_local(name, src, vbase + i as u32, read_only)?;
        }
        self.reserve(vars.len() as u32)?;
        // the loop's code after the body is made on its `end`
        self.block_scoped(body, TokRef::StatAfter(sid, -1))?;
        self.leave_block()
    }

    pub(super) fn generic_for(
        &mut self,
        sid: StatId,
        vars: &'a [Name],
        exprs: &[ExprId],
        body: &'a Block,
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
        let hidden: &[&'a str] = match layout {
            ForLayout::Gen53 => &["(for generator)", "(for state)", "(for control)"],
            ForLayout::Gen54 => &["(for state)"; 4],
            _ => &["(for state)"; 3],
        };
        // 5.5: the control (first) variable is read-only
        let mut names: LVec<(&'a str, NameSrc, bool)> = LVec::new(self.heap.mem());
        for (i, v) in vars.iter().enumerate() {
            let read_only = i == 0 && self.version >= LuaVersion::Lua55;
            names.push_or_abort((self.nm(v), NameSrc::Sym(v.sym), read_only));
        }
        self.log_for_names(sid, hidden, &names)?;
        let base = self.explist_adjust(exprs, values)?;
        let last = *exprs.last().expect("an expression list");
        self.forlist_room(base, values, exprs.len() as u32, layout.call_end())?;
        self.enter_block(true);
        self.declare_hidden(base, hidden)?;
        self.forlist_block(base, layout.closing().is_some());
        let vbase = base + layout.var();
        self.set_freereg(vbase);
        // `forbody`, once `do` is read
        self.at(TokRef::After(last, 1));
        let prep = self.emit(Inst::iabx(prep_op, base, 0));
        let nvars = vars.len() as u32;
        let body_top = self.here();
        self.for_body(sid, &names, vbase, body)?;
        let tforcall_pc = self.here();
        let skip = tforcall_pc - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.near_err("control structure too long"));
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
            return Err(self.near_err("control structure too long"));
        }
        if self.version == LuaVersion::Lua51 {
            self.last_line = body_last;
        }
        self.emit(Inst::iabx(loop_op, base, back as u32));
        self.mark_target(body_top);
        // the loop's block ends once its `end` is read
        if let Some(line) = end_line {
            self.last_line = line;
        }
        self.at(TokRef::StatAfter(sid, 0));
        self.leave_block()?;
        self.set_freereg(base);
        Ok(())
    }

    /// `luaK_checkstack`'s room to call the iterator, past the `values`
    /// registers from `base` (5.1 and 5.2: past the extra values of the
    /// `nexprs` expressions too, kept until the statement ends).
    fn forlist_room(
        &mut self,
        base: u32,
        values: u32,
        nexprs: u32,
        call_end: u32,
    ) -> Result<(), SyntaxError> {
        let extra = match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 => nexprs.saturating_sub(values),
            _ => 0,
        };
        self.set_freereg(base + values + extra);
        self.reserve(call_end - values)?;
        self.set_freereg(base + values);
        Ok(())
    }

    /// The generic loop's block: PUC enters it before the expressions, so
    /// its level is the base; with a closing value, a `return f()` in the
    /// body is no tail call and a `goto` leaving the loop closes it.
    fn forlist_block(&mut self, base: u32, closing: bool) {
        let b = self.l().blocks.last_mut().expect("no block");
        b.reg_floor = base;
        if closing {
            b.tbc_scope = true;
            b.has_tbc = true;
        }
    }

    pub(super) fn numeric_for(
        &mut self,
        sid: StatId,
        var: &Name,
        line: u32,
        (start, limit, step): (ExprId, ExprId, Option<ExprId>),
        body: &'a Block,
        end_line: Option<u32>,
    ) -> Result<(), SyntaxError> {
        self.last_line = line;
        let hidden: &[&'a str] = match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 => {
                &["(for index)", "(for limit)", "(for step)"]
            }
            LuaVersion::Lua55 => &["(for state)"; 2],
            _ => &["(for state)"; 3],
        };
        let v55 = self.version >= LuaVersion::Lua55;
        let var = (self.nm(var), NameSrc::Sym(var.sym), v55);
        self.log_for_names(sid, hidden, &[var])?;
        let base = self.lr().freereg;
        self.set_freereg(base);
        let se = self.expr(start)?;
        // a float start or step makes a float loop from 5.3 on (5.1/5.2
        // step every loop in floating point already)
        let mut float_loop = matches!(se, Exp::Float(_));
        // each value goes to its register once the token after it is read
        self.at(TokRef::After(start, 0));
        self.set_freereg(base);
        let s0 = self.exp_to_nextreg(se)?;
        debug_assert_eq!(s0, base);
        let le = self.expr(limit)?;
        self.at(TokRef::After(limit, 0));
        self.set_freereg(base + 1);
        let l0 = self.exp_to_nextreg(le)?;
        debug_assert_eq!(l0, base + 1);
        match step {
            Some(st) => {
                let ste = self.expr(st)?;
                self.at(TokRef::After(st, 0));
                float_loop |= matches!(ste, Exp::Float(_));
                self.set_freereg(base + 2);
                let st0 = self.exp_to_nextreg(ste)?;
                debug_assert_eq!(st0, base + 2);
            }
            None => {
                self.set_freereg(base + 2);
                self.reserve(1)?;
                // up to 5.3 the step 1 is a constant (`luaK_codek`): a
                // float before 5.3, which has no integers
                let one = if self.version <= LuaVersion::Lua52 {
                    Value::Float(1.0)
                } else {
                    Value::Int(1)
                };
                self.load_imm(base + 2, one, Op::LoadI, 1);
            }
        }
        // 5.5 keeps the index in the loop variable: its `forprep` drops one
        // of the three registers the values took
        let layout = if v55 {
            ForLayout::Num55
        } else {
            ForLayout::Num
        };
        let (prep_op, _, loop_op) = layout.ops();
        let loop_op = match loop_op {
            Op::ForLoop if float_loop && self.version >= LuaVersion::Lua53 => Op::ForLoopF,
            Op::ForLoop55 if float_loop => Op::ForLoop55F,
            op => op,
        };
        self.set_freereg(base + 3);
        self.enter_block(true);
        self.l().blocks.last_mut().expect("no block").reg_floor = base;
        self.declare_hidden(base, hidden)?;
        self.last_line = line;
        let var_reg = base + layout.var();
        self.set_freereg(var_reg);
        // `forbody`, once `do` is read
        self.at(TokRef::After(step.unwrap_or(limit), 1));
        let prep = self.emit(Inst::iabx(prep_op, base, 0));
        let body_top = self.here();
        self.for_body(sid, &[var], var_reg, body)?;
        let loop_pc = self.here();
        let back = loop_pc - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.near_err("control structure too long"));
        }
        // PUC attributes FORLOOP (the per-iteration back-edge) to the `for` line,
        // so each loop iteration re-fires a line event there.
        self.last_line = line;
        self.emit(Inst::iabx(loop_op, base, back as u32));
        let skip = self.here() - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.near_err("control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(prep_op, base, skip as u32);
        // ForLoop's back-edge lands at `body_top`; ForPrep's forward-skip
        // lands at the post-loop pc (= `here()` after the ForLoop emit).
        self.mark_target(body_top);
        let post_loop = self.here();
        self.mark_target(post_loop);
        if let Some(line) = end_line {
            self.last_line = line;
        }
        self.at(TokRef::StatAfter(sid, 0));
        self.leave_block()?;
        self.set_freereg(base);
        Ok(())
    }
}
