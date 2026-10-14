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
        id: ExprId,
        key: ExprId,
        saved: u32,
    },
    Call {
        id: ExprId,
        args: List<ExprId>,
        line: u32,
        base: u32,
    },
    Method {
        id: ExprId,
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
                        self.spine.push_or_abort(Pending::Index {
                            id: cur,
                            key,
                            saved,
                        });
                        cur = obj;
                    }
                },
                Expr::Call { func, args, line } => {
                    let base = self.lr().freereg;
                    self.spine.push_or_abort(Pending::Call {
                        id: cur,
                        args,
                        line,
                        base,
                    });
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
                        id: cur,
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
                Pending::Index { id, key, saved } => self.index_close(id, e, key, saved)?,
                Pending::Call {
                    id,
                    args,
                    line,
                    base,
                } => self.call_close(id, e, args, line, base)?,
                Pending::Method {
                    id,
                    method,
                    args,
                    line,
                    base,
                } => self.method_close(id, e, method, args, line, base)?,
            };
        }
        Ok(e)
    }

    /// An expression that is not a node of a left spine (see [`Self::expr`]).
    fn expr_leaf(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        let ast = self.ast;
        // PUC's scanner is past a simple expression when its code is made;
        // `...` makes its code on the token, a `function`, a table and an
        // operator along the way
        if !matches!(
            ast.expr(id),
            Expr::Vararg | Expr::Function(_) | Expr::Table { .. } | Expr::BinOp { .. }
        ) {
            self.at(TokRef::After(id, 0));
        }
        match ast.expr(id) {
            Expr::Nil => Ok(Exp::Nil),
            Expr::True => Ok(Exp::True),
            Expr::False => Ok(Exp::False),
            Expr::Int(i) => Ok(Exp::Int(*i)),
            Expr::Float(f) => Ok(Exp::Float(*f)),
            Expr::Str(s) => Ok(Exp::Const(self.literal_const(*s))),
            Expr::Name(n) => {
                self.last_line = n.line;
                // PUC emits the load of an upvalue or a global only when
                // the value is used
                self.early_emit = true;
                let e = self.name_expr(self.nm(n));
                self.early_emit = false;
                e
            }
            Expr::Paren(inner) => {
                // parentheses truncate multiple results to exactly one
                let e = self.expr(*inner)?;
                self.at(TokRef::After(id, 0));
                if let Exp::Open { .. } = e {
                    Ok(Exp::Reg(self.exp_to_anyreg(e)?))
                } else {
                    Ok(e)
                }
            }
            Expr::UnOp { op, operand, line } => {
                let (op, operand, line) = (*op, *operand, *line);
                self.unop(id, op, operand, line)
            }
            Expr::BinOp { lhs, rhs, line, .. } => {
                let (lhs, rhs, line) = (*lhs, *rhs, *line);
                self.concat(lhs, rhs, line)
            }
            Expr::Table { line, .. } => {
                let line = *line;
                self.table_ctor(id, line)
            }
            Expr::Vararg => {
                self.at(TokRef::After(id, -1));
                let e = self.vararg_expr();
                self.at(TokRef::After(id, 0));
                e
            }
            Expr::Function(body) => {
                let f = self.function_exp(body, false, TokRef::Open(id, 0), TokRef::After(id, 0));
                self.at(TokRef::After(id, 0));
                f
            }
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
}
