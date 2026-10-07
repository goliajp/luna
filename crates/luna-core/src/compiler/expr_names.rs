//! Names: locals, upvalues, globals through `_ENV`.

use super::*;

impl Compiler<'_> {
    pub(super) fn name_expr(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        match self.resolve_name(name)? {
            VarKind::Local(reg) => Ok(Exp::Reg(reg)),
            VarKind::Const(v) => Ok(self.ct_exp(v)),
            VarKind::Upval(u) => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetUpval,
                0,
                u,
                0,
                false,
            )))),
            VarKind::Global { .. } => {
                // declaration check (5.5): undeclared names error under a
                // strict regime; reads are fine for const globals
                let line = self.last_line;
                self.resolve_global_kind(name, line)?;
                self.global_access(name)
            }
        }
    }

    /// `_ENV[name]` with `_ENV` resolved through the scope chain (it can be
    /// shadowed by a local or captured as an upvalue).
    /// True when `_ENV` itself has been pulled into a `global` declaration: any
    /// global access then needs `_ENV._ENV`, which is itself global — an error
    /// (PUC's `buildglobal` rejects a VGLOBAL environment).
    pub(super) fn env_is_global(&self) -> bool {
        self.levels.iter().rev().any(|lvl| {
            lvl.blocks
                .iter()
                .any(|b| b.gdecls.iter().any(|(n, _)| &**n == "_ENV"))
        })
    }

    /// Emit the runtime "already defined" guard for a defining `global` write:
    /// reads the current value of the global and errors (OP_ERRNNIL) if it is
    /// not nil. Only `global x = ...` and `global function x` use this.
    pub(super) fn emit_global_redef_check(&mut self, name: &str) -> Result<(), SyntaxError> {
        let saved = self.lr().freereg;
        let e = self.global_access(name)?;
        let r = self.exp_to_anyreg(e)?;
        let c = self.str_const(name.as_bytes());
        let bx = if c < MAX_BX { c + 1 } else { 0 };
        self.emit(Inst::iabx(Op::ErrNNil, r, bx));
        self.set_freereg(saved);
        Ok(())
    }

    pub(super) fn global_access(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        if self.env_is_global() {
            return Err(self.err(
                self.last_line,
                format!("_ENV is global when accessing variable '{name}'"),
            ));
        }
        if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            let t = match self.resolve_env()? {
                VarKind::Upval(u) => TabRef::Up(u),
                VarKind::Local(r) => TabRef::Reg(r),
                VarKind::Global { .. } | VarKind::Const(_) => {
                    unreachable!("resolve_env gives a register or an upvalue")
                }
            };
            let c = self.str_const(name.as_bytes());
            let saved = self.lr().freereg;
            let (t, k) = self.indexed(t, Exp::Const(c))?;
            let e = self.index_get(t, k);
            self.set_freereg(saved);
            return Ok(e);
        }
        let c = self.str_const(name.as_bytes());
        match self.resolve_env()? {
            VarKind::Upval(u) if c <= 0xFF => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetTabUp,
                0,
                u,
                c,
                true,
            )))),
            VarKind::Local(r) if c <= 0xFF => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetField,
                0,
                r,
                c,
                true,
            )))),
            env => {
                // rare: huge constant index — go through registers
                let er = self.reserve(2)?;
                match env {
                    VarKind::Upval(u) => {
                        self.emit(Inst::iabc(Op::GetUpval, er, u, 0, false));
                    }
                    VarKind::Local(r) => {
                        self.emit(Inst::iabc(Op::Move, er, r, 0, false));
                    }
                    VarKind::Global { .. } | VarKind::Const(_) => {
                        unreachable!("resolve_env gives a register or an upvalue")
                    }
                }
                self.load_const(er + 1, c);
                self.set_freereg(er);
                Ok(Exp::Reloc(self.emit(Inst::iabc(
                    Op::GetTable,
                    0,
                    er,
                    er + 1,
                    false,
                ))))
            }
        }
    }

    /// `_ENV` as the table a global access indexes. A compile-time constant
    /// `_ENV` is loaded into a register first (PUC `luaK_exp2anyregup`).
    pub(super) fn resolve_env(&mut self) -> Result<VarKind, SyntaxError> {
        if self.version == LuaVersion::Lua51 {
            return Ok(VarKind::Upval(0));
        }
        match self.resolve_name("_ENV")? {
            VarKind::Const(v) => {
                let e = self.ct_exp(v);
                Ok(VarKind::Local(self.exp_to_anyreg(e)?))
            }
            k => Ok(k),
        }
    }
}
