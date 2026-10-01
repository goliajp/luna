//! The same tree with another representation of its names: the parser
//! builds the tree with interned names, and the public entry points hand
//! out (or take in) the tree with owned ones.

use super::*;

impl<N> Chunk<N> {
    /// This chunk with every name replaced by `f` of it.
    pub(crate) fn map_names<M>(&self, f: &mut impl FnMut(&N) -> M) -> Chunk<M> {
        Chunk {
            exprs: self.exprs.iter().map(|e| e.map_names(f)).collect(),
            stats: self.stats.iter().map(|s| s.map_names(f)).collect(),
            stat_lines: self.stat_lines.clone(),
            block: self.block.clone(),
            end_line: self.end_line,
        }
    }
}

impl<N> AttribName<N> {
    fn map_names<M>(&self, f: &mut impl FnMut(&N) -> M) -> AttribName<M> {
        AttribName {
            name: f(&self.name),
            attrib: self.attrib,
        }
    }
}

impl<N> FuncBody<N> {
    fn map_names<M>(&self, f: &mut impl FnMut(&N) -> M) -> FuncBody<M> {
        FuncBody {
            params: self.params.iter().map(&mut *f).collect(),
            vararg: match &self.vararg {
                Vararg::None => Vararg::None,
                Vararg::Anonymous => Vararg::Anonymous,
                Vararg::Named(n) => Vararg::Named(f(n)),
            },
            block: self.block.clone(),
            line: self.line,
            end_line: self.end_line,
        }
    }
}

impl<N> Stat<N> {
    fn map_names<M>(&self, f: &mut impl FnMut(&N) -> M) -> Stat<M> {
        match self {
            Stat::Do(b) => Stat::Do(b.clone()),
            Stat::While { cond, body } => Stat::While {
                cond: *cond,
                body: body.clone(),
            },
            Stat::Repeat { body, cond } => Stat::Repeat {
                body: body.clone(),
                cond: *cond,
            },
            Stat::If { arms, else_body } => Stat::If {
                arms: arms.clone(),
                else_body: else_body.clone(),
            },
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => Stat::NumericFor {
                var: f(var),
                start: *start,
                limit: *limit,
                step: *step,
                body: body.clone(),
            },
            Stat::GenericFor {
                vars,
                exprs,
                body,
                expr_line,
            } => Stat::GenericFor {
                vars: vars.iter().map(&mut *f).collect(),
                exprs: exprs.clone(),
                body: body.clone(),
                expr_line: *expr_line,
            },
            Stat::Local {
                collective,
                names,
                exprs,
            } => Stat::Local {
                collective: *collective,
                names: names.iter().map(|n| n.map_names(f)).collect(),
                exprs: exprs.clone(),
            },
            Stat::Global {
                collective,
                names,
                exprs,
            } => Stat::Global {
                collective: *collective,
                names: names.iter().map(|n| n.map_names(f)).collect(),
                exprs: exprs.clone(),
            },
            Stat::GlobalAll { attrib } => Stat::GlobalAll { attrib: *attrib },
            Stat::Assign { targets, exprs } => Stat::Assign {
                targets: targets.clone(),
                exprs: exprs.clone(),
            },
            Stat::Call(e) => Stat::Call(*e),
            Stat::Function { name, body } => Stat::Function {
                name: FuncName {
                    base: f(&name.base),
                    path: name.path.iter().map(&mut *f).collect(),
                    method: name.method.as_ref().map(&mut *f),
                },
                body: body.map_names(f),
            },
            Stat::LocalFunction { name, body } => Stat::LocalFunction {
                name: f(name),
                body: body.map_names(f),
            },
            Stat::GlobalFunction { name, body } => Stat::GlobalFunction {
                name: f(name),
                body: body.map_names(f),
            },
            Stat::Return { exprs, line } => Stat::Return {
                exprs: exprs.clone(),
                line: *line,
            },
            Stat::Break { line } => Stat::Break { line: *line },
            Stat::Goto(n) => Stat::Goto(f(n)),
            Stat::Label(n) => Stat::Label(f(n)),
        }
    }
}

impl<N> Expr<N> {
    fn map_names<M>(&self, f: &mut impl FnMut(&N) -> M) -> Expr<M> {
        match self {
            Expr::Nil => Expr::Nil,
            Expr::True => Expr::True,
            Expr::False => Expr::False,
            Expr::Vararg => Expr::Vararg,
            Expr::Int(i) => Expr::Int(*i),
            Expr::Float(x) => Expr::Float(*x),
            Expr::Str(s) => Expr::Str(s.clone()),
            Expr::Name(n) => Expr::Name(f(n)),
            Expr::Index { obj, key } => Expr::Index {
                obj: *obj,
                key: *key,
            },
            Expr::Call { func, args, line } => Expr::Call {
                func: *func,
                args: args.clone(),
                line: *line,
            },
            Expr::MethodCall {
                obj,
                method,
                args,
                line,
            } => Expr::MethodCall {
                obj: *obj,
                method: f(method),
                args: args.clone(),
                line: *line,
            },
            Expr::Function(b) => Expr::Function(b.map_names(f)),
            Expr::Table { fields, line } => Expr::Table {
                fields: fields
                    .iter()
                    .map(|t| match t {
                        TableField::Item(e) => TableField::Item(*e),
                        TableField::Named(n, e) => TableField::Named(f(n), *e),
                        TableField::Keyed(k, v) => TableField::Keyed(*k, *v),
                    })
                    .collect(),
                line: *line,
            },
            Expr::BinOp { op, lhs, rhs, line } => Expr::BinOp {
                op: *op,
                lhs: *lhs,
                rhs: *rhs,
                line: *line,
            },
            Expr::UnOp { op, operand, line } => Expr::UnOp {
                op: *op,
                operand: *operand,
                line: *line,
            },
            Expr::Paren(e) => Expr::Paren(*e),
        }
    }
}
