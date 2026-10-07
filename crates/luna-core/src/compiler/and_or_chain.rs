//! `a or b or c …` (and the `and` chain): every test jumps straight to
//! the end, as PUC's jump lists make it, instead of to the next test.

use super::*;

impl Compiler<'_> {
    /// The chain `lhs op rhs` when `lhs` is itself an `op` and every operand
    /// but the last is a plain value (a variable, an indexing, a call or
    /// `...`); `None` for any other shape, left to `and_or`.
    pub(super) fn and_or_chain(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Option<Exp>, SyntaxError> {
        let ast = self.ast;
        // the operands in source order, and the line of the operator after
        // each one but the last
        let mut operands: LVec<ExprId> = LVec::new(self.heap.mem());
        let mut lines: LVec<u32> = LVec::new(self.heap.mem());
        let mut stack: LVec<(ExprId, Option<u32>)> = LVec::new(self.heap.mem());
        stack.push_or_abort((rhs, None));
        stack.push_or_abort((lhs, Some(line)));
        while let Some((id, after)) = stack.pop() {
            match *ast.expr(id) {
                Expr::BinOp {
                    op: o,
                    lhs: l,
                    rhs: r,
                    line: ln,
                } if o == op => {
                    stack.push_or_abort((r, after));
                    stack.push_or_abort((l, Some(ln)));
                }
                _ => {
                    operands.push_or_abort(id);
                    if let Some(ln) = after {
                        lines.push_or_abort(ln);
                    }
                }
            }
        }
        let plain = |id: ExprId| {
            matches!(
                ast.expr(id),
                Expr::Name(_)
                    | Expr::Index { .. }
                    | Expr::Call { .. }
                    | Expr::MethodCall { .. }
                    | Expr::Vararg
            )
        };
        if operands.len() < 3 || !operands[..operands.len() - 1].iter().all(|&id| plain(id)) {
            return Ok(None);
        }
        let reg = self.lr().freereg;
        let mut jumps: LVec<usize> = LVec::new(self.heap.mem());
        let (last, tested) = operands.split_last().expect("three operands");
        for (&id, &ln) in tested.iter().zip(lines.iter()) {
            self.set_freereg(reg);
            let e = self.expr(id)?;
            self.set_freereg(reg);
            self.exp_to_nextreg(e)?;
            self.last_line = ln;
            self.emit(Inst::iabc(Op::Test, reg, 0, 0, op == BinOp::Or));
            jumps.push_or_abort(self.emit_jump());
        }
        self.set_freereg(reg);
        let e = self.expr(*last)?;
        self.set_freereg(reg);
        self.exp_to_nextreg(e)?;
        for &j in jumps.iter() {
            self.patch_to_here(j)?;
        }
        Ok(Some(Exp::Reg(reg)))
    }
}
