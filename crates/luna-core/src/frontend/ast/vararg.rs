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
pub fn block_uses_vararg(chunk: &Chunk, block: &Block) -> bool {
    chunk
        .block_stats(block)
        .iter()
        .any(|&sid| stat_uses_vararg(chunk, chunk.stat(sid)))
}

fn stat_uses_vararg(chunk: &Chunk, stat: &Stat) -> bool {
    use Stat::*;
    match stat {
        Do(b) => block_uses_vararg(chunk, b),
        While { cond, body } => expr_uses_vararg(chunk, *cond) || block_uses_vararg(chunk, body),
        Repeat { body, cond } => block_uses_vararg(chunk, body) || expr_uses_vararg(chunk, *cond),
        If { arms, else_body } => {
            chunk
                .list(*arms)
                .iter()
                .any(|a| expr_uses_vararg(chunk, a.cond) || block_uses_vararg(chunk, &a.body))
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
            chunk
                .list(*exprs)
                .iter()
                .any(|&e| expr_uses_vararg(chunk, e))
                || block_uses_vararg(chunk, body)
        }
        Local { exprs, .. } | Global { exprs, .. } => chunk
            .list(*exprs)
            .iter()
            .any(|&e| expr_uses_vararg(chunk, e)),
        GlobalAll { .. } => false,
        Assign { targets, exprs } => {
            chunk
                .list(*targets)
                .iter()
                .any(|&e| expr_uses_vararg(chunk, e))
                || chunk
                    .list(*exprs)
                    .iter()
                    .any(|&e| expr_uses_vararg(chunk, e))
        }
        Call(e) => expr_uses_vararg(chunk, *e),
        // Nested functions own their own vararg context — don't peek
        // inside them. (PUC's `simpleexp` only clears NEEDSARG on
        // direct `...` use in the current function's source.)
        Function { .. } | LocalFunction { .. } | GlobalFunction { .. } => false,
        Return { exprs, .. } => chunk
            .list(*exprs)
            .iter()
            .any(|&e| expr_uses_vararg(chunk, e)),
        Break { .. } | Goto(_) | Label(_) => false,
    }
}

fn expr_uses_vararg(chunk: &Chunk, eid: ExprId) -> bool {
    if crate::native_stack::is_low(crate::native_stack::RESERVE) {
        return true;
    }
    // every expression to look at; no recursion, so a long chain is no
    // deeper
    let mut todo: Vec<ExprId> = vec![eid];
    while let Some(e) = todo.pop() {
        match chunk.expr(e) {
            Expr::Vararg => return true,
            // Stop at function literals — their `...` is scoped to them.
            Expr::Function(_) => {}
            Expr::Index { obj, key } => todo.extend([*obj, *key]),
            Expr::Call { func, args, .. } => {
                todo.push(*func);
                todo.extend_from_slice(chunk.list(*args));
            }
            Expr::MethodCall { obj, args, .. } => {
                todo.push(*obj);
                todo.extend_from_slice(chunk.list(*args));
            }
            Expr::Table { fields, .. } => {
                for f in chunk.list(*fields) {
                    match f {
                        TableField::Item(e) | TableField::Named(_, e) => todo.push(*e),
                        TableField::Keyed(k, v) => todo.extend([*k, *v]),
                    }
                }
            }
            Expr::BinOp { lhs, rhs, .. } => todo.extend([*lhs, *rhs]),
            Expr::UnOp { operand, .. } => todo.push(*operand),
            Expr::Paren(inner) => todo.push(*inner),
            Expr::Nil
            | Expr::True
            | Expr::False
            | Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Name(_) => {}
        }
    }
    false
}
