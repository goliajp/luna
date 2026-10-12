//! Named-vararg materialization pre-scan.
//!
//! A named vararg `...t` can stay a virtual stack view (no heap table) as
//! long as every use is a read `t[k]` / `t.n`. Any write, bare use (passed,
//! returned, an operand, a method receiver) or capture by a nested function
//! forces a real table. These walkers conservatively return `true` (force)
//! on anything but a read-index of the name.

use super::*;

impl Compiler<'_> {
    pub(super) fn vararg_forced(&self, block: &ast::Block, name: &str) -> bool {
        self.ls(block.stats)
            .iter()
            .any(|&s| self.stat_forces(s, name))
    }

    fn block_forces(&self, block: &ast::Block, name: &str) -> bool {
        self.ls(block.stats)
            .iter()
            .any(|&s| self.stat_forces(s, name))
    }

    fn stat_forces(&self, s: StatId, name: &str) -> bool {
        use ast::Stat::*;
        match self.ast.stat(s) {
            Do(b) => self.block_forces(b, name),
            While { cond, body } => {
                self.expr_forces(*cond, name, false) || self.block_forces(body, name)
            }
            Repeat { body, cond } => {
                self.block_forces(body, name) || self.expr_forces(*cond, name, false)
            }
            If { arms, else_body } => {
                self.ls(*arms).iter().any(|a| {
                    self.expr_forces(a.cond, name, false) || self.block_forces(&a.body, name)
                }) || else_body
                    .as_ref()
                    .is_some_and(|b| self.block_forces(b, name))
            }
            NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.expr_forces(*start, name, false)
                    || self.expr_forces(*limit, name, false)
                    || step.is_some_and(|e| self.expr_forces(e, name, false))
                    || self.block_forces(body, name)
            }
            GenericFor { exprs, body, .. } => {
                self.ls(*exprs)
                    .iter()
                    .any(|&e| self.expr_forces(e, name, false))
                    || self.block_forces(body, name)
            }
            Local { exprs, .. } | Global { exprs, .. } => self
                .ls(*exprs)
                .iter()
                .any(|&e| self.expr_forces(e, name, false)),
            GlobalAll { .. } | Break { .. } | Goto(_) | Label(_) => false,
            Assign { targets, exprs } => {
                self.ls(*targets)
                    .iter()
                    .any(|&t| self.target_forces(t, name))
                    || self
                        .ls(*exprs)
                        .iter()
                        .any(|&e| self.expr_forces(e, name, false))
            }
            Call(e) => self.expr_forces(*e, name, false),
            // a nested function capturing the name escapes the vararg
            Function { body, .. } | LocalFunction { body, .. } | GlobalFunction { body, .. } => {
                self.mentions_block(&body.block, name)
            }
            Return { exprs, .. } => self
                .ls(*exprs)
                .iter()
                .any(|&e| self.expr_forces(e, name, false)),
        }
    }

    /// An assignment target: an `Index` write to the name (or assigning to the
    /// name itself) forces materialization.
    fn target_forces(&self, t: ExprId, name: &str) -> bool {
        match self.ast.expr(t) {
            Expr::Index { obj, key } => {
                self.expr_forces(*obj, name, false) || self.expr_forces(*key, name, false)
            }
            Expr::Name(n) => self.nm(n) == name,
            _ => self.expr_forces(t, name, false),
        }
    }

    /// `is_index_obj` is true when `e` is the object slot of a *read* index — the
    /// one position where a bare reference to the vararg is allowed to stay
    /// virtual.
    fn expr_forces(&self, e: ExprId, name: &str, is_index_obj: bool) -> bool {
        if crate::native_stack::is_low(crate::native_stack::RESERVE) {
            return true;
        }
        use ast::Expr::*;
        // every expression to look at, with whether it is the object of a
        // read index; no recursion, so a long chain is no deeper
        let mut todo: Vec<(ExprId, bool)> = vec![(e, is_index_obj)];
        while let Some((e, is_index_obj)) = todo.pop() {
            let forces = match self.ast.expr(e) {
                Name(n) => self.nm(n) == name && !is_index_obj,
                Index { obj, key } => {
                    todo.push((*obj, true));
                    todo.push((*key, false));
                    false
                }
                Call { func, args, .. } => {
                    todo.push((*func, false));
                    todo.extend(self.ls(*args).iter().map(|&a| (a, false)));
                    false
                }
                MethodCall { obj, args, .. } => {
                    todo.push((*obj, false));
                    todo.extend(self.ls(*args).iter().map(|&a| (a, false)));
                    false
                }
                BinOp { lhs, rhs, .. } => {
                    todo.push((*lhs, false));
                    todo.push((*rhs, false));
                    false
                }
                UnOp { operand, .. } => {
                    todo.push((*operand, false));
                    false
                }
                Paren(inner) => {
                    todo.push((*inner, false));
                    false
                }
                Table { fields, .. } => {
                    for f in self.ls(*fields) {
                        match f {
                            ast::TableField::Item(e) | ast::TableField::Named(_, e) => {
                                todo.push((*e, false));
                            }
                            ast::TableField::Keyed(k, v) => {
                                todo.push((*k, false));
                                todo.push((*v, false));
                            }
                        }
                    }
                    false
                }
                Function(body) => self.mentions_block(&body.block, name),
                Nil | True | False | Vararg | Int(_) | Float(_) | Str(_) => false,
            };
            if forces {
                return true;
            }
        }
        false
    }

    /// Whether `name` appears *anywhere* inside a (nested) block — any mention
    /// means the vararg is captured as an upvalue, forcing materialization.
    fn mentions_block(&self, block: &ast::Block, name: &str) -> bool {
        self.ls(block.stats)
            .iter()
            .any(|&s| self.mentions_stat(s, name))
    }

    fn mentions_stat(&self, s: StatId, name: &str) -> bool {
        use ast::Stat::*;
        match self.ast.stat(s) {
            Do(b) => self.mentions_block(b, name),
            While { cond, body } => {
                self.mentions_expr(*cond, name) || self.mentions_block(body, name)
            }
            Repeat { body, cond } => {
                self.mentions_block(body, name) || self.mentions_expr(*cond, name)
            }
            If { arms, else_body } => {
                self.ls(*arms)
                    .iter()
                    .any(|a| self.mentions_expr(a.cond, name) || self.mentions_block(&a.body, name))
                    || else_body
                        .as_ref()
                        .is_some_and(|b| self.mentions_block(b, name))
            }
            NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.mentions_expr(*start, name)
                    || self.mentions_expr(*limit, name)
                    || step.is_some_and(|e| self.mentions_expr(e, name))
                    || self.mentions_block(body, name)
            }
            GenericFor { exprs, body, .. } => {
                self.ls(*exprs).iter().any(|&e| self.mentions_expr(e, name))
                    || self.mentions_block(body, name)
            }
            Local { exprs, .. } | Global { exprs, .. } | Return { exprs, .. } => {
                self.ls(*exprs).iter().any(|&e| self.mentions_expr(e, name))
            }
            Assign { targets, exprs } => {
                self.ls(*targets)
                    .iter()
                    .any(|&e| self.mentions_expr(e, name))
                    || self.ls(*exprs).iter().any(|&e| self.mentions_expr(e, name))
            }
            Call(e) => self.mentions_expr(*e, name),
            Function { body, .. } | LocalFunction { body, .. } | GlobalFunction { body, .. } => {
                self.mentions_block(&body.block, name)
            }
            GlobalAll { .. } | Break { .. } | Goto(_) | Label(_) => false,
        }
    }

    fn mentions_expr(&self, e: ExprId, name: &str) -> bool {
        if crate::native_stack::is_low(crate::native_stack::RESERVE) {
            return true;
        }
        use ast::Expr::*;
        let mut todo: Vec<ExprId> = vec![e];
        while let Some(e) = todo.pop() {
            let mentions = match self.ast.expr(e) {
                Name(n) => self.nm(n) == name,
                Index { obj, key } => {
                    todo.extend([*obj, *key]);
                    false
                }
                Call { func, args, .. } => {
                    todo.push(*func);
                    todo.extend_from_slice(self.ls(*args));
                    false
                }
                MethodCall { obj, args, .. } => {
                    todo.push(*obj);
                    todo.extend_from_slice(self.ls(*args));
                    false
                }
                BinOp { lhs, rhs, .. } => {
                    todo.extend([*lhs, *rhs]);
                    false
                }
                UnOp { operand, .. } => {
                    todo.push(*operand);
                    false
                }
                Paren(inner) => {
                    todo.push(*inner);
                    false
                }
                Table { fields, .. } => {
                    for f in self.ls(*fields) {
                        match f {
                            ast::TableField::Item(e) | ast::TableField::Named(_, e) => {
                                todo.push(*e);
                            }
                            ast::TableField::Keyed(k, v) => todo.extend([*k, *v]),
                        }
                    }
                    false
                }
                Function(body) => self.mentions_block(&body.block, name),
                Nil | True | False | Vararg | Int(_) | Float(_) | Str(_) => false,
            };
            if mentions {
                return true;
            }
        }
        false
    }
}
