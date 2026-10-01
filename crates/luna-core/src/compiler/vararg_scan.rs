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
        block.stats.iter().any(|&s| self.stat_forces(s, name))
    }

    fn block_forces(&self, block: &ast::Block, name: &str) -> bool {
        block.stats.iter().any(|&s| self.stat_forces(s, name))
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
                arms.iter().any(|(c, _, b)| {
                    self.expr_forces(*c, name, false) || self.block_forces(b, name)
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
                exprs.iter().any(|&e| self.expr_forces(e, name, false))
                    || self.block_forces(body, name)
            }
            Local { exprs, .. } | Global { exprs, .. } => {
                exprs.iter().any(|&e| self.expr_forces(e, name, false))
            }
            GlobalAll { .. } | Break { .. } | Goto(_) | Label(_) => false,
            Assign { targets, exprs } => {
                targets.iter().any(|&t| self.target_forces(t, name))
                    || exprs.iter().any(|&e| self.expr_forces(e, name, false))
            }
            Call(e) => self.expr_forces(*e, name, false),
            // a nested function capturing the name escapes the vararg
            Function { body, .. } | LocalFunction { body, .. } | GlobalFunction { body, .. } => {
                self.mentions_block(&body.block, name)
            }
            Return { exprs, .. } => exprs.iter().any(|&e| self.expr_forces(e, name, false)),
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
        use ast::Expr::*;
        match self.ast.expr(e) {
            Name(n) => self.nm(n) == name && !is_index_obj,
            Index { obj, key } => {
                self.expr_forces(*obj, name, true) || self.expr_forces(*key, name, false)
            }
            Call { func, args, .. } => {
                self.expr_forces(*func, name, false)
                    || args.iter().any(|&a| self.expr_forces(a, name, false))
            }
            MethodCall { obj, args, .. } => {
                self.expr_forces(*obj, name, false)
                    || args.iter().any(|&a| self.expr_forces(a, name, false))
            }
            BinOp { lhs, rhs, .. } => {
                self.expr_forces(*lhs, name, false) || self.expr_forces(*rhs, name, false)
            }
            UnOp { operand, .. } => self.expr_forces(*operand, name, false),
            Paren(inner) => self.expr_forces(*inner, name, false),
            Table { fields, .. } => fields.iter().any(|f| self.field_forces(f, name)),
            Function(body) => self.mentions_block(&body.block, name),
            Nil | True | False | Vararg | Int(_) | Float(_) | Str(_) => false,
        }
    }

    fn field_forces(&self, f: &TableField, name: &str) -> bool {
        match f {
            ast::TableField::Item(e) => self.expr_forces(*e, name, false),
            ast::TableField::Named(_, e) => self.expr_forces(*e, name, false),
            ast::TableField::Keyed(k, v) => {
                self.expr_forces(*k, name, false) || self.expr_forces(*v, name, false)
            }
        }
    }

    /// Whether `name` appears *anywhere* inside a (nested) block — any mention
    /// means the vararg is captured as an upvalue, forcing materialization.
    fn mentions_block(&self, block: &ast::Block, name: &str) -> bool {
        block.stats.iter().any(|&s| self.mentions_stat(s, name))
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
                arms.iter()
                    .any(|(c, _, b)| self.mentions_expr(*c, name) || self.mentions_block(b, name))
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
                exprs.iter().any(|&e| self.mentions_expr(e, name))
                    || self.mentions_block(body, name)
            }
            Local { exprs, .. } | Global { exprs, .. } | Return { exprs, .. } => {
                exprs.iter().any(|&e| self.mentions_expr(e, name))
            }
            Assign { targets, exprs } => {
                targets.iter().any(|&e| self.mentions_expr(e, name))
                    || exprs.iter().any(|&e| self.mentions_expr(e, name))
            }
            Call(e) => self.mentions_expr(*e, name),
            Function { body, .. } | LocalFunction { body, .. } | GlobalFunction { body, .. } => {
                self.mentions_block(&body.block, name)
            }
            GlobalAll { .. } | Break { .. } | Goto(_) | Label(_) => false,
        }
    }

    fn mentions_expr(&self, e: ExprId, name: &str) -> bool {
        use ast::Expr::*;
        match self.ast.expr(e) {
            Name(n) => self.nm(n) == name,
            Index { obj, key } => self.mentions_expr(*obj, name) || self.mentions_expr(*key, name),
            Call { func, args, .. } => {
                self.mentions_expr(*func, name) || args.iter().any(|&a| self.mentions_expr(a, name))
            }
            MethodCall { obj, args, .. } => {
                self.mentions_expr(*obj, name) || args.iter().any(|&a| self.mentions_expr(a, name))
            }
            BinOp { lhs, rhs, .. } => {
                self.mentions_expr(*lhs, name) || self.mentions_expr(*rhs, name)
            }
            UnOp { operand, .. } => self.mentions_expr(*operand, name),
            Paren(inner) => self.mentions_expr(*inner, name),
            Table { fields, .. } => fields.iter().any(|f| match f {
                ast::TableField::Item(e) => self.mentions_expr(*e, name),
                ast::TableField::Named(_, e) => self.mentions_expr(*e, name),
                ast::TableField::Keyed(k, v) => {
                    self.mentions_expr(*k, name) || self.mentions_expr(*v, name)
                }
            }),
            Function(body) => self.mentions_block(&body.block, name),
            Nil | True | False | Vararg | Int(_) | Float(_) | Str(_) => false,
        }
    }
}
