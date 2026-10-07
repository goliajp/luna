//! Function bodies: compiling a nested function into a closure.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn function_exp(
        &mut self,
        body: &'a FuncBody,
        is_method: bool,
    ) -> Result<Exp, SyntaxError> {
        let line = body.line;
        let nparams = body.params.len as usize + is_method as usize;
        if nparams > 200 {
            return Err(self.err(line, "too many parameters"));
        }
        let is_vararg = !matches!(body.vararg, ast::Vararg::None);
        let mut level = self.new_level(nparams as u8, is_vararg, line);
        // PUC 5.5 `parlist`: emit a hidden `(vararg table)` locvar only for
        // an explicit anonymous `(...)` (Named goes through a real local;
        // main chunks set is_vararg implicitly with no pseudo). 5.4 and
        // earlier had no such pseudo — db.lua across versions baselines on
        // the exact `getlocal` shift: 5.5 setlocal(2, 4) = "AAAA" vs
        // 5.4/5.3/5.2 setlocal(2, 3) = "AAAA".
        level.has_vararg_table_pseudo =
            self.version >= LuaVersion::Lua55 && matches!(body.vararg, ast::Vararg::Anonymous);
        // PUC 5.1 attached an `env` slot to *every* Lua function so
        // `setfenv` always had something to rewrite, even for bodies that
        // never touched a global. luna keeps it as a hidden `_ENV` upvalue,
        // seeded eagerly as upvalue 0 of every 5.1 function (the main chunk's
        // upvalue 0 too), so it inherits the creator's upvalue 0. 5.2+ keeps
        // the lazy-capture model.
        if self.version == LuaVersion::Lua51 {
            level.upvals.push_or_abort(UpvalDesc {
                in_stack: false,
                index: 0,
                name: "_ENV".into(),
                read_only: false,
            });
        }
        self.levels.push_or_abort(level);
        self.enter_block(false);
        if is_method {
            self.declare_local("self", 0, false)?;
        }
        for (i, p) in self.ls(body.params).iter().enumerate() {
            self.declare_local(self.nm(p), (i + is_method as usize) as u32, false)?;
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
        self.stat_block(&body.block)?;
        // PUC attributes the implicit final return to the closing `end` line, so
        // that line shows up in `debug.getinfo(...,"L").activelines`.
        self.final_return(body.end_line)?;
        let lvl = self.levels.pop().expect("function level");
        let proto = self.finish_level(lvl, line, body.end_line);
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
            return Ok(Exp::Reg(self.exp_to_nextreg(Exp::Reloc(pc))?));
        }
        Ok(Exp::Reloc(pc))
    }
}
