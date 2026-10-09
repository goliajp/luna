//! Expression dispatch and calls (names are in `expr_names`, discharging
//! into registers in `discharge`).

use super::binop::BinOpOpen;
use super::expr_ops::IndexOpen;
use super::*;

/// A node of an expression's left spine whose left child is being
/// compiled: what finishing it needs besides that child's value.
pub(super) enum Pending {
    BinOp {
        op: BinOp,
        rhs: ExprId,
        line: u32,
        open: BinOpOpen,
    },
    AndOr {
        op: BinOp,
        rhs: ExprId,
        line: u32,
    },
    Index {
        key: ExprId,
        saved: u32,
    },
    Call {
        args: List<ExprId>,
        line: u32,
        base: u32,
    },
    Method {
        method: Name,
        args: List<ExprId>,
        line: u32,
        base: u32,
    },
}

impl<'a> Compiler<'a> {
    /// Compile an expression. The nodes whose left child is compiled first
    /// (a binary operator's left operand, an index's object, a call's
    /// function) form a left spine that a long chain (`1 + 1 + ... + 1`,
    /// `a.b.b.b`, `f()()()`) makes as deep as the source is long: the spine
    /// is walked down and back up without recursion, as PUC's parser emits
    /// such chains in a loop.
    pub(super) fn expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        if crate::native_stack::is_low(crate::native_stack::RESERVE) {
            return Err(self.too_deep());
        }
        let ast = self.ast;
        // the spine of this expression sits above `mark` on the shared stack
        let mark = self.spine.len();
        let mut cur = id;
        let mut e = loop {
            match *ast.expr(cur) {
                Expr::BinOp {
                    op: op @ (BinOp::And | BinOp::Or),
                    lhs,
                    rhs,
                    line,
                } => {
                    self.spine.push_or_abort(Pending::AndOr { op, rhs, line });
                    cur = lhs;
                }
                Expr::BinOp { op, lhs, rhs, line } if op != BinOp::Concat => {
                    let (open, le) = self.binop_open(op, lhs, line)?;
                    self.spine.push_or_abort(Pending::BinOp {
                        op,
                        rhs,
                        line,
                        open,
                    });
                    match le {
                        Some(le) => break le,
                        None => cur = lhs,
                    }
                }
                Expr::Index { obj, key } => match self.index_open(obj, key)? {
                    IndexOpen::Done(e) => break e,
                    IndexOpen::Object { saved } => {
                        self.spine.push_or_abort(Pending::Index { key, saved });
                        cur = obj;
                    }
                },
                Expr::Call { func, args, line } => {
                    let base = self.lr().freereg;
                    self.spine.push_or_abort(Pending::Call { args, line, base });
                    cur = func;
                }
                Expr::MethodCall {
                    obj,
                    method,
                    args,
                    line,
                } => {
                    let base = self.lr().freereg;
                    self.spine.push_or_abort(Pending::Method {
                        method,
                        args,
                        line,
                        base,
                    });
                    cur = obj;
                }
                _ => break self.expr_leaf(cur)?,
            }
        };
        while self.spine.len() > mark {
            let p = self.spine.pop().expect("spine entry");
            e = match p {
                Pending::BinOp {
                    op,
                    rhs,
                    line,
                    open,
                } => self.binop_close(op, e, rhs, line, open)?,
                Pending::AndOr { op, rhs, line } => self.and_or_close(op, e, rhs, line)?,
                Pending::Index { key, saved } => self.index_close(e, key, saved)?,
                Pending::Call { args, line, base } => self.call_close(e, args, line, base)?,
                Pending::Method {
                    method,
                    args,
                    line,
                    base,
                } => self.method_close(e, method, args, line, base)?,
            };
        }
        Ok(e)
    }

    /// An expression that is not a node of a left spine (see [`Self::expr`]).
    fn expr_leaf(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        let ast = self.ast;
        match ast.expr(id) {
            Expr::Nil => Ok(Exp::Nil),
            Expr::True => Ok(Exp::True),
            Expr::False => Ok(Exp::False),
            Expr::Int(i) => Ok(Exp::Int(*i)),
            Expr::Float(f) => Ok(Exp::Float(*f)),
            Expr::Str(s) => Ok(Exp::Const(self.sym_const(*s))),
            Expr::Name(n) => {
                self.last_line = n.line;
                self.name_expr(self.nm(n))
            }
            Expr::Paren(inner) => {
                // parentheses truncate multiple results to exactly one
                let e = self.expr(*inner)?;
                if let Exp::Open { .. } = e {
                    Ok(Exp::Reg(self.exp_to_anyreg(e)?))
                } else {
                    Ok(e)
                }
            }
            Expr::UnOp { op, operand, line } => {
                let (op, operand, line) = (*op, *operand, *line);
                self.unop(op, operand, line)
            }
            Expr::BinOp { lhs, rhs, line, .. } => {
                let (lhs, rhs, line) = (*lhs, *rhs, *line);
                self.concat(lhs, rhs, line)
            }
            Expr::Table { line, .. } => {
                let line = *line;
                self.table_ctor(id, line)
            }
            Expr::Vararg => self.vararg_expr(),
            Expr::Function(body) => self.function_exp(body, false),
            Expr::Index { .. } | Expr::Call { .. } | Expr::MethodCall { .. } => {
                unreachable!("a left-spine node")
            }
        }
    }

    pub(super) fn vararg_expr(&mut self) -> Result<Exp, SyntaxError> {
        if !self.lr().is_vararg {
            return Err(self.err(self.last_line, "cannot use '...' outside a vararg function"));
        }
        let base = self.reserve(1)?;
        let pc = self.emit(Inst::iabc(Op::Vararg, base, 0, 2, false));
        Ok(Exp::Open { pc, base })
    }

    /// `f(args)`, its function `fe` compiled with `base` the free register
    /// before it.
    fn call_close(
        &mut self,
        fe: Exp,
        args: List<ExprId>,
        line: u32,
        base: u32,
    ) -> Result<Exp, SyntaxError> {
        self.set_freereg(base);
        let r = self.exp_to_nextreg(fe)?;
        debug_assert_eq!(r, base);
        let (nfixed, open) = self.args_onto_stack(self.ls(args), base + 1)?;
        self.last_line = line;
        let b = if open { 0 } else { nfixed + 1 };
        let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
        self.set_freereg(base + 1);
        Ok(Exp::Open { pc, base })
    }

    /// `obj:method(args)`, its receiver `oe` compiled with `base` the free
    /// register before it.
    fn method_close(
        &mut self,
        oe: Exp,
        method: Name,
        args: List<ExprId>,
        line: u32,
        base: u32,
    ) -> Result<Exp, SyntaxError> {
        let o = self.exp_to_anyreg(oe)?;
        self.set_freereg(base);
        self.reserve(2)?;
        let c = self.sym_const(method.sym);
        self.last_line = line;
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
        let (nfixed, open) = self.args_onto_stack(self.ls(args), base + 2)?;
        self.last_line = line;
        let b = if open { 0 } else { nfixed + 2 };
        let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
        self.set_freereg(base + 1);
        Ok(Exp::Open { pc, base })
    }

    /// Stack call arguments at consecutive registers from `argbase`.
    /// Returns (fixed_arg_count, last_is_open).
    pub(super) fn args_onto_stack(
        &mut self,
        args: &[ExprId],
        argbase: u32,
    ) -> Result<(u32, bool), SyntaxError> {
        for (i, &a) in args.iter().enumerate() {
            let dst = argbase + i as u32;
            if dst >= max_regs(self.version) {
                // PUC `checkstack` raises "function or expression needs too
                // many registers" once the per-function register cap is hit;
                // a too-wide call site is just one path into it (errors.lua
                // :740 checkmessage "too many registers").
                return Err(self.regs_error(self.last_line));
            }
            self.set_freereg(dst);
            let last = i == args.len() - 1;
            let e = self.expr(a)?;
            if last && let Exp::Open { pc, base } = e {
                debug_assert_eq!(base, dst);
                self.patch_wanted(pc, 0);
                return Ok((args.len() as u32 - 1, true));
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        Ok((args.len() as u32, false))
    }
}
