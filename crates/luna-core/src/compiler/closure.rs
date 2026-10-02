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
            level.upvals.push(UpvalDesc {
                in_stack: false,
                index: 0,
                name: "_ENV".into(),
                read_only: false,
            });
        }
        self.levels.push(level);
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
        // PUC 5.1's `LUA_COMPAT_VARARG` reserves the *name* `arg` as a hidden
        // local at index numparams+1 in every vararg function. Whether the
        // slot ends up populated as a table is a separate question that
        // vararg.lua contradicts itself on (`:6` wants `arg` to be a table
        // inside `function f(a, ...)` while `:13` wants `arg == nil` inside
        // `function c12 (...)`); luna leaves the slot at its register-init
        // value (nil) so the `arg == nil` half still passes — db.lua's
        // `setlocal(2, 3, "pera") == "AAAA"` only depends on the *numbering*
        // shifting by 1 to make AAAA land at local index 3, which the name
        // reservation alone delivers.
        // PUC 5.1 LUAI_COMPAT_VARARG: `(...)` functions get a hidden
        // `arg` local UNLESS the body uses `...` directly (lparser.c
        // singlevar: `simpleexp` clears VARARG_NEEDSARG on `TK_DOTS`).
        // vararg.lua relies on this: `function f(a, ...) … arg.n …
        // end` uses `arg` (no `...`) → auto-bound; `function c12 (...)
        // local x = {...}` uses `...` → no auto-`arg` (assert(arg ==
        // nil) sees the GLOBAL arg which was reset to nil at file
        // top).
        if self.version <= LuaVersion::Lua51
            && matches!(body.vararg, ast::Vararg::Anonymous)
            && !block_uses_vararg(self.ast, &body.block)
        {
            let r = self.reserve(1)?;
            self.declare_local("arg", r, false)?;
            self.l().has_compat_vararg_arg = true;
        }
        self.stat_block(&body.block)?;
        self.leave_block()?;
        // PUC attributes the implicit final return to the closing `end` line, so
        // that line shows up in `debug.getinfo(...,"L").activelines`.
        self.last_line = body.end_line;
        self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
        let lvl = self.levels.pop().expect("function level");
        let proto = self.finish_level(lvl, line, body.end_line);
        let idx = self.lr().protos.len() as u32;
        if idx > MAX_BX {
            return Err(self.err(line, "too many nested functions"));
        }
        self.l().protos.push(proto);
        // PUC emits OP_CLOSURE with the line of the just-consumed `end` token
        // (luaK_code uses ls->lastline), so the closure-creation line event lands
        // on the function's last line, not its `function` keyword.
        self.last_line = body.end_line;
        Ok(Exp::Reloc(self.emit(Inst::iabx(Op::Closure, 0, idx))))
    }
}
