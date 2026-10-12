//! Unary operators, `and` / `or`, concatenation and indexing.

use super::*;

/// What [`Compiler::index_open`] found.
pub(super) enum IndexOpen {
    /// the index compiled, needing no object
    Done(Exp),
    /// the object is to be compiled, `saved` the free register before it
    Object { saved: u32 },
}

impl<'a> Compiler<'a> {
    pub(super) fn unop(
        &mut self,
        op: UnOp,
        operand: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        self.last_line = line;
        let e = self.expr(operand)?;
        let saved = self.lr().freereg;
        let (opcode, folded) = match op {
            UnOp::Neg | UnOp::BNot => {
                let n = match e {
                    Exp::Int(i) => Some(Num::Int(i)),
                    Exp::Float(f) => Some(Num::Float(f)),
                    _ => None,
                };
                match n.and_then(|n| fold::fold_unary(op, n, self.version)) {
                    Some(Num::Int(i)) => return Ok(Exp::Int(i)),
                    Some(Num::Float(f)) => return Ok(Exp::Float(f)),
                    None if op == UnOp::Neg => (Op::Unm, e),
                    None => (Op::BNot, e),
                }
            }
            UnOp::Not => {
                let e = self.discharge_vars(e);
                return self.code_not(e);
            }
            UnOp::Len => (Op::Len, e),
        };
        let r = self.exp_to_anyreg(folded)?;
        self.set_freereg(saved);
        self.last_line = line;
        Ok(Exp::Reloc(self.emit(Inst::iabc(opcode, 0, r, 0, false))))
    }

    /// `and` / `or` with the left operand compiled to `le` (PUC
    /// `luaK_infix` and `luaK_posfix`): the left operand's test goes into
    /// the list of the value that decides the whole, and the right operand
    /// takes over the rest.
    pub(super) fn and_or_close(
        &mut self,
        op: BinOp,
        le: Exp,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        // PUC tests the left operand once it has read the operator
        self.last_line = line;
        let le = if op == BinOp::And {
            self.go_if_true(le)?
        } else {
            self.go_if_false(le)?
        };
        let re = self.expr(rhs)?;
        let (v, mut t, mut f) = self.exp_parts(re);
        let v = self.discharge_vars(v);
        let (_, lt, lf) = self.exp_parts(le);
        if op == BinOp::And {
            self.concat_list(&mut f, lf)?;
        } else {
            self.concat_list(&mut t, lt)?;
        }
        Ok(self.exp_with(v, t, f))
    }

    pub(super) fn concat(
        &mut self,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        // PUC `luaK_concat` collapses a right-associative `a..b..c..d..…`
        // chain into a single OP_CONCAT with `b = nargs`. luna previously
        // emitted one OP_CONCAT per binary, building a chain of pair-folds;
        // a 128-operand chain (5.1 big.lua's `rep129(longs)`) then needed
        // dozens of intern+hash rounds over multi-GB intermediates before
        // hitting `concat_pair`'s overflow check. Flatten upfront so the
        // run-side pre-sum can short-circuit the whole expression.
        //
        // Collect the right-associative chain's operands left-to-right.
        // a concatenation among the operands finds the buffer taken and
        // makes its own
        let mut operands = self.operands.take();
        operands.push_or_abort(lhs);
        let mut cur = rhs;
        loop {
            match self.ast.expr(cur) {
                Expr::BinOp {
                    op: BinOp::Concat,
                    lhs: l,
                    rhs: r,
                    ..
                } => {
                    operands.push_or_abort(*l);
                    cur = *r;
                }
                _ => {
                    operands.push_or_abort(cur);
                    break;
                }
            }
        }
        let base = self.lr().freereg;
        let mut nargs = 0u32;
        for (idx, &eid) in operands.iter().enumerate() {
            self.set_freereg(base + nargs);
            let e = self.expr(eid)?;
            self.set_freereg(base + nargs);
            let r = self.exp_to_nextreg(e)?;
            debug_assert_eq!(r, base + nargs);
            nargs += 1;
            // OP_CONCAT's `b` field is one byte (`b = nargs`), capping the
            // chain at 254 fixed operands. Anything beyond closes the
            // current segment and starts a fresh outer concat with the
            // running result as the new lhs.
            let is_last = idx + 1 == operands.len();
            if !is_last && nargs == 254 {
                self.set_freereg(base);
                self.last_line = line;
                self.emit(Inst::iabc(Op::Concat, base, nargs, 0, false));
                nargs = 1;
            }
        }
        operands.clear();
        self.operands = operands;
        self.set_freereg(base);
        self.last_line = line;
        // 5.1–5.3 `CONCAT` names its destination, which is left to the use
        if self.version <= LuaVersion::Lua53 {
            return Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::Concat,
                0,
                nargs,
                base,
                true,
            ))));
        }
        self.emit(Inst::iabc(Op::Concat, base, nargs, 0, false));
        Ok(Exp::Reg(base))
    }

    /// True when `name` resolves, in the current function, to a named vararg
    /// kept virtual (so `name[k]` indexes the stack varargs via OP_VARGIDX).
    pub(super) fn vararg_virtual_local(&self, name: &str) -> bool {
        let lvl = self.lr();
        // a more-recent `global name` marker shadows the local
        if lvl.global_declared(name) {
            return false;
        }
        lvl.locals
            .iter()
            .rposition(|l| l.name == name)
            .is_some_and(|idx| lvl.locals[idx].vararg_virtual)
    }

    /// Before the object of `obj[key]` is compiled: the whole index when it
    /// needs no object, else the free register to put back.
    pub(super) fn index_open(
        &mut self,
        obj: ExprId,
        key: ExprId,
    ) -> Result<IndexOpen, SyntaxError> {
        let ast = self.ast;
        // a read `t[k]` / `t.n` of a virtual named vararg: index the stack
        // varargs directly (OP_VARGIDX), allocating no table.
        if let Expr::Name(n) = ast.expr(obj)
            && self.vararg_virtual_local(self.nm(n))
        {
            let saved = self.lr().freereg;
            let ke = self.expr(key)?;
            let k = self.exp_to_anyreg(ke)?;
            let e = Exp::Reloc(self.emit(Inst::iabc(Op::VargIdx, 0, 0, k, false)));
            self.set_freereg(saved);
            return Ok(IndexOpen::Done(e));
        }
        Ok(IndexOpen::Object {
            saved: self.lr().freereg,
        })
    }

    /// `obj[key]` with the object compiled to `oe`.
    pub(super) fn index_close(
        &mut self,
        oe: Exp,
        key: ExprId,
        saved: u32,
    ) -> Result<Exp, SyntaxError> {
        let t = self.index_table(oe)?;
        let ke = self.expr(key)?;
        let (t, k) = self.indexed(t, ke)?;
        let e = self.index_get(t, k);
        self.set_freereg(saved);
        Ok(e)
    }
}
