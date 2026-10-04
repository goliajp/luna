//! Local declarations and name resolution (locals, upvalues, globals).

use super::*;
use crate::runtime::mem::Oom;

impl<'a> Compiler<'a> {
    /// 5.5 global-declaration resolution: explicit declaration > innermost
    /// collective `global *` > implicit chunk default (void once any
    /// declaration is in scope).
    pub(super) fn resolve_global_kind(
        &mut self,
        name: &str,
        line: u32,
    ) -> Result<VarKind, SyntaxError> {
        let mut innermost_collective: Option<bool> = None;
        let mut any_decl = false;
        for lvl in self.levels.iter().rev() {
            for b in lvl.blocks.iter().rev() {
                if let Some(&(_, ro)) = b.gdecls.iter().rev().find(|&&(n, _)| n == name) {
                    return Ok(VarKind::Global { read_only: ro });
                }
                if innermost_collective.is_none()
                    && let Some(ro) = b.collective
                {
                    innermost_collective = Some(ro);
                }
                any_decl |= !b.gdecls.is_empty() || b.collective.is_some();
            }
        }
        if let Some(ro) = innermost_collective {
            return Ok(VarKind::Global { read_only: ro });
        }
        if any_decl {
            return Err(self.err(line, format!("variable '{name}' not declared")));
        }
        Ok(VarKind::Global { read_only: false })
    }

    /// The innermost block needs a CLOSE on its back-edge/exit paths when it
    /// captured locals or declared to-be-closed ones.
    pub(super) fn block_captured(&self) -> bool {
        let b = self.lr().blocks.last().expect("no block");
        b.has_tbc || self.lr().locals[b.first_local..].iter().any(|l| l.captured)
    }

    pub(super) fn block_floor(&self) -> u32 {
        self.lr().blocks.last().expect("no block").reg_floor
    }

    pub(super) fn declare_local(
        &mut self,
        name: &'a str,
        reg: u32,
        read_only: bool,
    ) -> Result<(), SyntaxError> {
        // PUC `new_localvar` calls `checklimit(fs, …, MAXVARS, "local variables")`
        // before recording the slot — luna counts active avars (skip global
        // markers and any pending vararg pseudo) to model the same cap.
        let active = self.lr().avars.iter().filter(|a| a.reg.is_some()).count() as u32;
        if active >= MAX_LOCALS {
            return Err(self.limit_err("local variables", MAX_LOCALS));
        }
        let start_pc = self.lr().code.len() as u32;
        self.l().locals.push(LocalVar {
            name,
            reg,
            read_only,
            captured: false,
            vararg_virtual: false,
            start_pc,
            konst: None,
        })?;
        self.l().avars.push(AVar {
            name: Some(name),
            reg: Some(reg),
            global: false,
        })?;
        Ok(())
    }

    /// Declare a compile-time constant local (PUC `RDKCTC`).
    pub(super) fn declare_ct_const(&mut self, name: &'a str, value: CtConst) -> Result<(), Oom> {
        let start_pc = self.lr().code.len() as u32;
        self.l().locals.push(LocalVar {
            name,
            reg: u32::MAX,
            read_only: true,
            captured: false,
            vararg_virtual: false,
            start_pc,
            konst: Some(value),
        })?;
        self.l().avars.push(AVar {
            name: Some(name),
            reg: None,
            global: false,
        })
    }

    /// The compile-time constant `name` refers to here, if it does: the
    /// nearest binding of the name, walking out through the functions, is
    /// a constant local. No upvalue is created on the way.
    pub(super) fn ct_const_named(&self, name: &str) -> Option<CtConst> {
        for lvl in self.levels.iter().rev() {
            if lvl
                .avars
                .iter()
                .rev()
                .find(|a| a.name == Some(name))
                .is_some_and(|a| a.global)
            {
                return None;
            }
            if let Some(l) = lvl.locals.iter().rev().find(|l| l.name == name) {
                return l.konst.clone();
            }
            if lvl.upvals.iter().any(|u| &*u.name == name) {
                return None;
            }
        }
        None
    }

    /// Materialise a compile-time constant as an expression of the
    /// function being compiled.
    pub(super) fn ct_exp(&mut self, v: CtConst) -> Result<Exp, Oom> {
        Ok(match v {
            CtConst::Nil => Exp::Nil,
            CtConst::Bool(true) => Exp::True,
            CtConst::Bool(false) => Exp::False,
            CtConst::Int(i) => Exp::Int(i),
            CtConst::Float(f) => Exp::Float(f),
            CtConst::Str(s) => Exp::Const(self.sym_const(s)?),
        })
    }

    /// Append a `global` declaration marker to the active-variable sequence so
    /// a goto jumping over it lands "into its scope" (PUC's `new_varkind` +
    /// `nactvar++`). `name` is `None` for a `global *` collective marker.
    pub(super) fn declare_global_marker(&mut self, name: Option<&'a str>) -> Result<(), Oom> {
        self.l().avars.push(AVar {
            name,
            reg: None,
            global: true,
        })
    }

    /// Register floor to CLOSE when discarding locals declared at/after the
    /// given avar index (the first real local in that suffix), if any.
    pub(super) fn reg_floor_from_avar(&self, avar_idx: usize) -> Option<u32> {
        self.lr().avars[avar_idx..].iter().find_map(|a| a.reg)
    }

    pub(super) fn resolve_name(&mut self, name: &str) -> Result<VarKind, SyntaxError> {
        let top = self.levels.len() - 1;
        self.resolve_at(top, name)
    }

    pub(super) fn resolve_at(&mut self, li: usize, name: &str) -> Result<VarKind, SyntaxError> {
        // innermost binding among this level's locals and explicit `global`
        // declarations (PUC's single `actvar` list): an inner `global X`
        // shadows an enclosing local X (and vice-versa). A `global *` marker
        // has no name and never matches here — collective scope is the
        // unbound-name fallback handled by resolve_global_kind.
        if let Some(av) = self.levels[li]
            .avars
            .iter()
            .rev()
            .find(|a| a.name == Some(name))
            && av.global
        {
            return Ok(VarKind::Global { read_only: false });
        }
        if let Some(idx) = self.levels[li].locals.iter().rposition(|l| l.name == name) {
            let local = &self.levels[li].locals[idx];
            return Ok(match &local.konst {
                Some(v) => VarKind::Const(v.clone()),
                None => VarKind::Local(local.reg),
            });
        }
        // in 5.1 `_ENV` is an ordinary name: a user's `_ENV` never resolves to
        // the hidden environment cell, which is every 5.1 function's upvalue 0
        let skip = usize::from(self.version == LuaVersion::Lua51 && name == "_ENV");
        let ups = &self.levels[li].upvals;
        if let Some(ui) = ups.iter().skip(skip).position(|u| &*u.name == name) {
            return Ok(VarKind::Upval((ui + skip) as u32));
        }
        if li == 0 {
            return Ok(VarKind::Global { read_only: false });
        }
        match self.resolve_at(li - 1, name)? {
            VarKind::Global { .. } => Ok(VarKind::Global { read_only: false }),
            // a constant needs no upvalue
            VarKind::Const(v) => Ok(VarKind::Const(v)),
            VarKind::Local(reg) => {
                let mut read_only = false;
                if let Some(idx) = self.levels[li - 1]
                    .locals
                    .iter()
                    .rposition(|l| l.reg == reg && l.name == name)
                {
                    self.levels[li - 1].locals[idx].captured = true;
                    read_only = self.levels[li - 1].locals[idx].read_only;
                }
                let ui = self.levels[li].upvals.len() as u32;
                if self.counted_upvals(li) >= max_upvals(self.version) {
                    return Err(self.limit_err_at(li, "upvalues", max_upvals(self.version)));
                }
                self.levels[li].upvals.push(UpvalDesc {
                    in_stack: true,
                    index: reg as u8,
                    name: name.into(),
                    read_only,
                })?;
                Ok(VarKind::Upval(ui))
            }
            VarKind::Upval(pidx) => {
                let read_only = self.levels[li - 1].upvals[pidx as usize].read_only;
                let ui = self.levels[li].upvals.len() as u32;
                if self.counted_upvals(li) >= max_upvals(self.version) {
                    return Err(self.limit_err_at(li, "upvalues", max_upvals(self.version)));
                }
                self.levels[li].upvals.push(UpvalDesc {
                    in_stack: false,
                    index: pidx as u8,
                    name: name.into(),
                    read_only,
                })?;
                Ok(VarKind::Upval(ui))
            }
        }
    }

    pub(super) fn local_is_read_only(&self, reg: u32) -> Option<&str> {
        self.lr()
            .locals
            .iter()
            .rev()
            .find(|l| l.reg == reg)
            .filter(|l| l.read_only)
            .map(|l| l.name)
    }
}
