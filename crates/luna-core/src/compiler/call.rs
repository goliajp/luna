//! Calls: the function or receiver, then the arguments, in consecutive
//! registers.

use super::*;

impl Compiler<'_> {
    /// `f(args)`, its function `fe` compiled with `base` the free register
    /// before it.
    pub(super) fn call_close(
        &mut self,
        id: ExprId,
        fe: Exp,
        args: List<ExprId>,
        line: u32,
        base: u32,
    ) -> Result<Exp, SyntaxError> {
        let Expr::Call { func, .. } = *self.ast.expr(id) else {
            unreachable!("a call")
        };
        self.at(TokRef::After(func, 0));
        self.set_freereg(base);
        let r = self.exp_to_nextreg(fe)?;
        debug_assert_eq!(r, base);
        let (nfixed, open) = self.args_onto_stack(id, self.ls(args), base + 1)?;
        self.last_line = line;
        let b = if open { 0 } else { nfixed + 1 };
        let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
        self.set_freereg(base + 1);
        Ok(Exp::Open { pc, base })
    }

    /// `obj:method(args)`, its receiver `oe` compiled with `base` the free
    /// register before it.
    pub(super) fn method_close(
        &mut self,
        id: ExprId,
        oe: Exp,
        method: Name,
        args: List<ExprId>,
        line: u32,
        base: u32,
    ) -> Result<Exp, SyntaxError> {
        let Expr::MethodCall { obj, .. } = *self.ast.expr(id) else {
            unreachable!("a method call")
        };
        // `luaK_self`, once `:` and the name are read
        self.at(TokRef::After(obj, 2));
        // before 5.4 the name is a constant as it is read (`checkname`),
        // before `luaK_self` puts the object in a register
        let named = (self.version <= LuaVersion::Lua53).then(|| {
            let from = self.puc_log.len();
            let c = self.sym_const(method.sym);
            self.log_before_early(from);
            c
        });
        let o = self.exp_to_anyreg(oe)?;
        self.set_freereg(base);
        self.reserve(2)?;
        let c = match named {
            Some(c) => c,
            None => self.sym_const(method.sym),
        };
        // `luaK_self` is coded once the method's name is read
        self.last_line = method.line;
        self.emit_self(base, o, c)?;
        let (nfixed, open) = self.args_onto_stack(id, self.ls(args), base + 2)?;
        self.last_line = line;
        let b = if open { 0 } else { nfixed + 2 };
        let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
        self.set_freereg(base + 1);
        Ok(Exp::Open { pc, base })
    }

    /// `luaK_self` of the object in register `o` and the method name, the
    /// constant `c`, into `base` and `base + 1`.
    fn emit_self(&mut self, base: u32, o: u32, c: u32) -> Result<(), SyntaxError> {
        if c <= 0xFF {
            self.emit(Inst::iabc(Op::SelfOp, base, o, c, true));
        } else if self.version >= LuaVersion::Lua55 {
            // PUC 5.5 drops back to a plain GETTABLE when the SELF
            // C-operand can't fit the constant — getobjname then
            // classifies the call as "field". 5.5 errors.lua :328
            // bakes the wording in (its comment literally says
            // "cannot use 'self' opcode").
            self.emit(Inst::iabc(Op::Move, base + 1, o, 0, false));
            let kr = self.reserve(1)?;
            self.load_const(kr, c);
            self.emit(Inst::iabc(Op::GetTable, base, base + 1, kr, false));
            self.set_freereg(base + 2);
        } else {
            // PUC 5.4 `luaK_exp2RK`: load the key into a register and
            // emit OP_SELF against it (k=false). The SELF tag stays on
            // the instruction so getobjname classifies the call as
            // "method" — 5.4 errors.lua :303 exercises this path.
            let kr = self.reserve(1)?;
            self.load_const(kr, c);
            self.set_freereg(base);
            self.reserve(2)?;
            self.emit(Inst::iabc(Op::SelfOp, base, o, kr, false));
        }
        Ok(())
    }

    /// Stack the arguments of call `call` at consecutive registers from
    /// `argbase`. Returns (fixed_arg_count, last_is_open).
    pub(super) fn args_onto_stack(
        &mut self,
        call: ExprId,
        args: &[ExprId],
        argbase: u32,
    ) -> Result<(u32, bool), SyntaxError> {
        for (i, &a) in args.iter().enumerate() {
            let dst = argbase + i as u32;
            self.set_freereg(dst);
            let last = i == args.len() - 1;
            let e = self.expr(a)?;
            if last && let Exp::Open { pc, base } = e {
                debug_assert_eq!(base, dst);
                self.patch_wanted(pc, 0);
                self.at(TokRef::After(call, 0));
                return Ok((args.len() as u32 - 1, true));
            }
            // an argument goes to its register once the `,` after it is
            // read, the last once the call's `)` is
            self.at(if last {
                TokRef::After(call, 0)
            } else {
                TokRef::After(a, 1)
            });
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        self.at(TokRef::After(call, 0));
        Ok((args.len() as u32, false))
    }
}
