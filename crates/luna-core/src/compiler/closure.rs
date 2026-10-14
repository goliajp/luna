//! Function bodies: compiling a nested function into a closure, and the
//! `function` statements that store one.

use super::*;

impl<'a> Compiler<'a> {
    /// A function literal: `body`, opened (PUC `addprototype`) at token
    /// `open`, its `(`, and closed (PUC `close_func`) at token `close`, the
    /// one after its `end`.
    pub(super) fn function_exp(
        &mut self,
        body: &'a FuncBody,
        is_method: bool,
        open: TokRef,
        close: TokRef,
    ) -> Result<Exp, SyntaxError> {
        let line = body.line;
        let nparams = body.params.len as usize + is_method as usize;
        if nparams > 200 {
            return Err(self.err(line, "too many parameters"));
        }
        let is_vararg = !matches!(body.vararg, ast::Vararg::None);
        self.at(open);
        self.open_level(nparams as u8, is_vararg, line);
        let version = self.version;
        let level = self.l();
        // PUC 5.5 `parlist`: emit a hidden `(vararg table)` locvar only for
        // an explicit anonymous `(...)` (Named goes through a real local;
        // main chunks set is_vararg implicitly with no pseudo). 5.4 and
        // earlier had no such pseudo — db.lua across versions baselines on
        // the exact `getlocal` shift: 5.5 setlocal(2, 4) = "AAAA" vs
        // 5.4/5.3/5.2 setlocal(2, 3) = "AAAA".
        level.has_vararg_table_pseudo =
            version >= LuaVersion::Lua55 && matches!(body.vararg, ast::Vararg::Anonymous);
        // PUC 5.1 attached an `env` slot to *every* Lua function so
        // `setfenv` always had something to rewrite, even for bodies that
        // never touched a global. luna keeps it as a hidden `_ENV` upvalue,
        // seeded eagerly as upvalue 0 of every 5.1 function (the main chunk's
        // upvalue 0 too), so it inherits the creator's upvalue 0. 5.2+ keeps
        // the lazy-capture model.
        if version == LuaVersion::Lua51 {
            level.upvals.push_or_abort(UpvalDesc {
                in_stack: false,
                index: 0,
                name: "_ENV".into(),
                read_only: false,
            });
        }
        self.enter_block(false);
        // the scanner past `(`: `self`, entered at once
        if is_method {
            self.at(open.plus(1));
            self.declare_local("self", 0, false)?;
        }
        // each parameter is declared once its name is read; their debug
        // records come at `)` (PUC `parlist`'s `adjustlocalvars`)
        self.puc_state.defer = true;
        let params = self.ls(body.params);
        for (i, p) in params.iter().enumerate() {
            self.at(open.plus(2 + 2 * i as i32));
            self.declare_local(self.nm(p), (i + is_method as usize) as u32, false)?;
        }
        // 5.5 (PUC `parlist`): an anonymous `...` is a parameter too, with a
        // register after the fixed ones that holds nil
        if self.lr().has_vararg_table_pseudo {
            let r = self.reserve(1)?;
            self.declare_local("(vararg table)", r, false)?;
        }
        if let ast::Vararg::Named(n) = &body.vararg {
            let name: &str = self.nm(n);
            let r = self.reserve(1)?;
            // 5.5: the named vararg table is a read-only local. If the pre-scan
            // proves it is only ever read as `t[k]`/`t.n` (never written, never
            // escaping, not `_ENV`) it stays *virtual* — indexed straight off the
            // stack varargs with no heap table. Otherwise materialize it now.
            let virtual_ok = name != "_ENV" && !self.vararg_forced(&body.block, name);
            if virtual_ok {
                self.declare_local(name, r, true)?;
                self.l()
                    .locals
                    .last_mut()
                    .expect("just declared")
                    .vararg_virtual = true;
            } else {
                self.emit(Inst::iabc(Op::GetVarg, r, 0, 0, false));
                self.declare_local(name, r, true)?;
            }
        }
        // PUC 5.1's `LUA_COMPAT_VARARG` declares a local `arg` after the
        // fixed parameters of every `(...)` function. It holds a table of the
        // extra arguments (`VARARG_NEEDSARG`) only when the body does not use
        // `...` itself (lparser.c `simpleexp` clears the flag on `TK_DOTS`);
        // otherwise it is nil, and it still hides a global `arg`.
        if self.version <= LuaVersion::Lua51 && matches!(body.vararg, ast::Vararg::Anonymous) {
            let r = self.reserve(1)?;
            self.declare_local("arg", r, false)?;
            if !block_uses_vararg(self.ast, &body.block) {
                self.l().has_compat_vararg_arg = true;
            }
        }
        // `)`: past the names and their commas, and `...` with its comma
        let n = params.len() as i32;
        let dots = match body.vararg {
            ast::Vararg::None => 0,
            _ if n > 0 => 2,
            _ => 1,
        };
        self.at(open.plus(1 + (2 * n - 1).max(0) + dots));
        self.flush_locvars();
        if is_vararg {
            self.log(super::puc_log::PEv::Prologue);
        }
        self.stat_block(&body.block)?;
        self.at(close);
        // PUC attributes the implicit final return to the closing `end` line, so
        // that line shows up in `debug.getinfo(...,"L").activelines`.
        let final_ret = self.puc_log.len();
        self.final_return(body.end_line)?;
        let proto = self.finish_level(line, body.end_line);
        let idx = self.lr().protos.len() as u32;
        if idx > MAX_BX {
            return Err(self.err(line, "too many nested functions"));
        }
        self.l().protos.push_or_abort(proto);
        // PUC emits OP_CLOSURE with the line of the just-consumed `end` token
        // (luaK_code uses ls->lastline), so the closure-creation line event lands
        // on the function's last line, not its `function` keyword.
        self.last_line = body.end_line;
        let pc = self.emit(Inst::iabx(Op::Closure, 0, idx));
        // 5.2+ `codeclosure` puts the closure in the next register at once
        if self.version >= LuaVersion::Lua52 {
            self.close_after_closure(final_ret);
            return Ok(Exp::Reg(self.exp_to_nextreg(Exp::Reloc(pc))?));
        }
        Ok(Exp::Reloc(pc))
    }

    pub(super) fn function_stat(
        &mut self,
        sid: StatId,
        name: &FuncName,
        body: &'a FuncBody,
    ) -> Result<(), SyntaxError> {
        self.last_line = name.base.line;
        let is_method = name.method.is_some();
        let saved = self.lr().freereg;
        // PUC `funcstat` compiles the name first, then the body, then the
        // store. Every GETFIELD / SETFIELD on a dotted name, and the store,
        // carry the line of the statement's name, not the `end` token's, so
        // a `nil` holder raises on the right line (errors.lua :430).
        let saved_force = self.force_line.replace(name.base.line);
        let lv = self.func_name_lv(sid, name);
        self.force_line = saved_force;
        let lv = lv?;
        let f = self.function_exp(
            body,
            is_method,
            TokRef::StatMark(sid, 0),
            TokRef::StatAfter(sid, 0),
        )?;
        let saved_force = self.force_line.replace(name.base.line);
        let res = self.store(lv, f);
        self.force_line = saved_force;
        res?;
        self.set_freereg(saved);
        Ok(())
    }

    /// PUC `funcname`: `a.b.c:m` as an assignment target, the statement
    /// `sid`'s.
    fn func_name_lv(&mut self, sid: StatId, name: &FuncName) -> Result<Lv, SyntaxError> {
        // `function`, the name: then a `.` or `:` and a name per field
        self.at(TokRef::StatFirst(sid, 2));
        let mut fields: LVec<&str> = LVec::new(self.heap.mem());
        for n in self.ls(name.path) {
            fields.push_or_abort(self.nm(n));
        }
        if let Some(m) = &name.method {
            fields.push_or_abort(self.nm(m));
        }
        let Some((last, walk)) = fields.split_last() else {
            return self.name_lv(self.nm(&name.base), name.base.line);
        };
        self.early_emit = true;
        let mut e = self.name_expr(self.nm(&name.base));
        self.early_emit = false;
        let mut at = 2;
        for f in walk {
            // the holder's register is free again once it is read
            let mark = self.lr().freereg;
            self.at(TokRef::StatFirst(sid, at));
            let t = self.index_table(e?)?;
            let c = self.str_const(f.as_bytes());
            self.at(TokRef::StatFirst(sid, at + 2));
            let (t, k) = self.indexed(t, Exp::Const(c))?;
            self.early_emit = true;
            e = Ok(self.index_get(t, k));
            self.early_emit = false;
            self.set_freereg(mark);
            at += 2;
        }
        self.at(TokRef::StatFirst(sid, at));
        let t = self.index_table(e?)?;
        self.at(TokRef::StatFirst(sid, at + 2));
        let c = self.str_const(last.as_bytes());
        let (t, k) = self.indexed(t, Exp::Const(c))?;
        Ok(Lv::Indexed(t, k))
    }
}
