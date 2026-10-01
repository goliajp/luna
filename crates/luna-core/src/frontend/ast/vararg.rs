//! Whether a block uses `...` (the 5.1 `arg` heuristic).

use super::*;

/// Does any expression in `block` (and nested control-flow,
/// but NOT nested `Expr::Function` bodies) use `Expr::Vararg`?
///
/// PUC 5.1 `LUAI_COMPAT_VARARG` heuristic: a `(...)` function gets a
/// hidden `arg` local UNLESS the body references `...`. The clear of
/// `VARARG_NEEDSARG` in lparser.c happens at `simpleexp`'s TK_DOTS
/// branch, which is a body-level decision. luna's compiler now runs
/// this AST walk before declaring the auto-`arg` local.
pub fn block_uses_vararg<N>(chunk: &Chunk<N>, block: &Block) -> bool {
    block
        .stats
        .iter()
        .any(|&sid| stat_uses_vararg(chunk, chunk.stat(sid)))
}

fn stat_uses_vararg<N>(chunk: &Chunk<N>, stat: &Stat<N>) -> bool {
    use Stat::*;
    match stat {
        Do(b) => block_uses_vararg(chunk, b),
        While { cond, body } => expr_uses_vararg(chunk, *cond) || block_uses_vararg(chunk, body),
        Repeat { body, cond } => block_uses_vararg(chunk, body) || expr_uses_vararg(chunk, *cond),
        If { arms, else_body } => {
            arms.iter()
                .any(|(c, _, b)| expr_uses_vararg(chunk, *c) || block_uses_vararg(chunk, b))
                || else_body
                    .as_ref()
                    .is_some_and(|b| block_uses_vararg(chunk, b))
        }
        NumericFor {
            start,
            limit,
            step,
            body,
            ..
        } => {
            expr_uses_vararg(chunk, *start)
                || expr_uses_vararg(chunk, *limit)
                || step.is_some_and(|s| expr_uses_vararg(chunk, s))
                || block_uses_vararg(chunk, body)
        }
        GenericFor { exprs, body, .. } => {
            exprs.iter().any(|&e| expr_uses_vararg(chunk, e)) || block_uses_vararg(chunk, body)
        }
        Local { exprs, .. } | Global { exprs, .. } => {
            exprs.iter().any(|&e| expr_uses_vararg(chunk, e))
        }
        GlobalAll { .. } => false,
        Assign { targets, exprs } => {
            targets.iter().any(|&e| expr_uses_vararg(chunk, e))
                || exprs.iter().any(|&e| expr_uses_vararg(chunk, e))
        }
        Call(e) => expr_uses_vararg(chunk, *e),
        // Nested functions own their own vararg context — don't peek
        // inside them. (PUC's `simpleexp` only clears NEEDSARG on
        // direct `...` use in the current function's source.)
        Function { .. } | LocalFunction { .. } | GlobalFunction { .. } => false,
        Return { exprs, .. } => exprs.iter().any(|&e| expr_uses_vararg(chunk, e)),
        Break { .. } | Goto(_) | Label(_) => false,
    }
}

fn expr_uses_vararg<N>(chunk: &Chunk<N>, eid: ExprId) -> bool {
    match chunk.expr(eid) {
        Expr::Vararg => true,
        // Stop at function literals — their `...` is scoped to them.
        Expr::Function(_) => false,
        Expr::Index { obj, key } => expr_uses_vararg(chunk, *obj) || expr_uses_vararg(chunk, *key),
        Expr::Call { func, args, .. } => {
            expr_uses_vararg(chunk, *func) || args.iter().any(|&a| expr_uses_vararg(chunk, a))
        }
        Expr::MethodCall { obj, args, .. } => {
            expr_uses_vararg(chunk, *obj) || args.iter().any(|&a| expr_uses_vararg(chunk, a))
        }
        Expr::Table { fields, .. } => fields.iter().any(|f| table_field_uses_vararg(chunk, f)),
        Expr::BinOp { lhs, rhs, .. } => {
            expr_uses_vararg(chunk, *lhs) || expr_uses_vararg(chunk, *rhs)
        }
        Expr::UnOp { operand, .. } => expr_uses_vararg(chunk, *operand),
        Expr::Paren(inner) => expr_uses_vararg(chunk, *inner),
        Expr::Nil
        | Expr::True
        | Expr::False
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Name(_) => false,
    }
}

fn table_field_uses_vararg<N>(chunk: &Chunk<N>, f: &TableField<N>) -> bool {
    match f {
        TableField::Item(e) | TableField::Named(_, e) => expr_uses_vararg(chunk, *e),
        TableField::Keyed(k, v) => expr_uses_vararg(chunk, *k) || expr_uses_vararg(chunk, *v),
    }
}
