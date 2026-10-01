//! When an indexed assignment may skip copying its table and key into
//! fresh registers.

use super::*;

impl Compiler<'_> {
    /// Compiler-side metamethod-safety gate for the Index-LHS object
    /// snapshot elision.
    ///
    /// Returns `true` when, for a single-target Index-LHS assignment
    /// `obj.key = rhs` (or `obj[key] = rhs`), the otherwise unconditional
    /// `exp_to_nextreg(oe)` snapshot in `assign_stat` is provably
    /// redundant.
    ///
    /// The four conditions enforced:
    ///
    /// 1. `targets.len() == 1` and `exprs.len() == 1` — no inter-target
    ///    or multi-RHS conflict possible.
    /// 2. The single target is `Expr::Index { obj: Name(local), .. }`
    ///    where the name resolves to a real local in the current level
    ///    (not an upvalue / global / read-only / vararg-virtual).
    /// 3. `locals[reg].captured == false` — no closure has captured
    ///    this local's slot, so no metatable-stored Lua closure can
    ///    rebind it through the upvalue.
    /// 4. AST-side
    ///    [`ast::metamethod_safe_for_index_lhs`][crate::frontend::ast::metamethod_safe_for_index_lhs]
    ///    over `(obj, exprs[0])` returns true (no UserOrUnknown RHS
    ///    calls; obj is a bare Name).
    ///
    /// Called from the Index-LHS branch of `assign_stat`.
    pub(crate) fn assign_stat_can_skip_obj_snapshot(
        &self,
        targets: &[ExprId],
        exprs: &[ExprId],
    ) -> bool {
        if targets.len() != 1 || exprs.len() != 1 {
            return false;
        }
        let (obj_eid, _key_eid) = match self.ast.expr(targets[0]) {
            Expr::Index { obj, key } => (*obj, *key),
            _ => return false,
        };
        let name_text = match self.ast.expr(obj_eid) {
            Expr::Name(n) => &*n.text,
            _ => return false,
        };
        // Resolve the name against the *current* level only — we
        // intentionally do not chase upvalues here because the elision only
        // covers snapshots for owner-level locals.
        let level = self.lr();
        let local = match level.locals.iter().find(|l| &*l.name == name_text) {
            Some(l) => l,
            None => return false,
        };
        if local.captured || local.vararg_virtual || local.konst.is_some() {
            return false;
        }
        // AST-side gate (call walker + obj-is-name check).
        ast::metamethod_safe_for_index_lhs(self.ast, obj_eid, exprs[0])
    }

    /// The key of a single `t[k] = e` can stay in its local's register (as
    /// in PUC, which reads the register when it stores) when `k` is a local
    /// of this function that no closure captures and `e` calls nothing
    /// unknown: nothing can then change the local before the store.
    pub(super) fn assign_stat_can_skip_key_snapshot(
        &self,
        targets: &[ExprId],
        exprs: &[ExprId],
    ) -> bool {
        if targets.len() != 1 || exprs.len() != 1 {
            return false;
        }
        let Expr::Index { key, .. } = self.ast.expr(targets[0]) else {
            return false;
        };
        let Expr::Name(n) = self.ast.expr(*key) else {
            return false;
        };
        let Some(local) = self.lr().locals.iter().rev().find(|l| l.name == n.text) else {
            return false;
        };
        !local.captured
            && !local.vararg_virtual
            && local.konst.is_none()
            && ast::rhs_calls_nothing_unknown(self.ast, exprs[0])
    }
}
