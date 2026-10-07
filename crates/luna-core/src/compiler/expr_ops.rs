//! Unary operators, `and` / `or`, concatenation and indexing.

use super::*;
use crate::runtime::mem::LVec;

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
            UnOp::Not => match e {
                Exp::Nil | Exp::False => return Ok(Exp::True),
                Exp::True | Exp::Int(_) | Exp::Float(_) | Exp::Const(_) => return Ok(Exp::False),
                e => (Op::Not, e),
            },
            UnOp::Len => (Op::Len, e),
        };
        let r = self.exp_to_anyreg(folded)?;
        self.set_freereg(saved);
        self.last_line = line;
        Ok(Exp::Reloc(self.emit(Inst::iabc(opcode, 0, r, 0, false))))
    }

    pub(super) fn and_or(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        if let Some(e) = self.and_or_chain(op, lhs, rhs, line)? {
            return Ok(e);
        }
        self.last_line = line;
        let base = self.lr().freereg;
        let le = self.expr(lhs)?;
        self.and_or_close(op, le, rhs, line, base)
    }

    /// `and` / `or` with the left operand compiled to `le`, `base` the free
    /// register before it.
    pub(super) fn and_or_close(
        &mut self,
        op: BinOp,
        le: Exp,
        rhs: ExprId,
        line: u32,
        base: u32,
    ) -> Result<Exp, SyntaxError> {
        // PUC's jumplist for `X and Y` / `X or Y` when X is a comparison:
        // skip materializing X to a bool — the comparison's own conditional
        // jump *is* the short-circuit. For AND, emit the Cmp with k=false so
        // its Jmp fires on FALSE (short-circuit-to-false); for OR, k=true so
        // the Jmp fires on TRUE (short-circuit-to-true). Then compile Y into
        // the result register and patch X's jump to land at the matching pad
        // of Y's materialization. db.lua :603 (count-hook ceiling) needs the
        // 3-op savings vs the legacy `materialize X → Test → Jmp` path.
        if let Exp::Cmp { op: cop, l, r, c } = le {
            let is_and = matches!(op, BinOp::And);
            // For AND, Jmp on cond==false (k=false). For OR, Jmp on cond==true (k=true).
            self.emit(Inst::iabc(cop, l, r, c, !is_and));
            let jmp_lhs = self.emit_jump();
            self.set_freereg(base);
            let re = self.expr(rhs)?;
            // RHS shapes that leak freereg += 1 (nested and/or, function call,
            // table ctor) would make the next `reserve(1)` return `base + 1`,
            // tripping the debug_assert and silently emitting `LFalseSkip` /
            // `LoadTrue` at `base + 1` in release — clobbering RHS's
            // temporary. Restore the invariant before reserving the result
            // slot, mirroring the non-Cmp branch below (lines 1786-1796).
            self.set_freereg(base);
            let reg = self.reserve(1)?;
            debug_assert_eq!(reg, base);
            // Materialize RHS into `reg` with the standard Cmp pad shape
            // (Lt + Jmp + LFalseSkip + LoadTrue) so X's short-circuit jump
            // can land on the matching pad slot.
            let (false_pad_pc, true_pad_pc) = match re {
                Exp::Cmp {
                    op: y_op,
                    l: y_l,
                    r: y_r,
                    c: y_c,
                } => {
                    self.emit(Inst::iabc(y_op, y_l, y_r, y_c, true));
                    self.emit(Inst::isj(Op::Jmp, 1));
                    let fpad = self.here();
                    self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                    let tpad = self.here();
                    self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                    // Jmp(1) skips LFalseSkip → tpad; LHS short-circuit will
                    // also patch into one of these pads below.
                    self.mark_target(tpad);
                    self.mark_target(fpad);
                    (fpad, tpad)
                }
                _ => {
                    // RHS is a regular value: materialize it normally, then
                    // emit an inline false/true pad after a skip jump so the
                    // LHS short-circuit lands on the matching constant.
                    self.set_freereg(reg);
                    self.exp_to_reg(re, reg)?;
                    let jmp_over = self.emit_jump();
                    let fpad = self.here();
                    self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                    let tpad = self.here();
                    self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                    self.patch_to_here(jmp_over)?;
                    // LHS short-circuit patches into fpad or tpad below.
                    self.mark_target(fpad);
                    self.mark_target(tpad);
                    (fpad, tpad)
                }
            };
            let target = if is_and { false_pad_pc } else { true_pad_pc };
            let off = target as i64 - jmp_lhs as i64 - 1;
            if off.unsigned_abs() > MAX_SJ as u64 {
                return Err(self.err(line, "control structure too long"));
            }
            self.l().code[jmp_lhs].set_sj(off as i32);
            return Ok(Exp::Reg(reg));
        }
        self.set_freereg(base);
        let reg = self.exp_to_nextreg(le)?;
        debug_assert_eq!(reg, base);
        let k = op == BinOp::Or;
        self.emit(Inst::iabc(Op::Test, reg, 0, 0, k));
        let jmp = self.emit_jump();
        self.set_freereg(reg);
        let re = self.expr(rhs)?;
        self.set_freereg(reg);
        let got = self.exp_to_nextreg(re)?;
        debug_assert_eq!(got, reg);
        self.patch_to_here(jmp)?;
        Ok(Exp::Reg(reg))
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
        let mut operands: LVec<ExprId> = LVec::new(self.heap.mem());
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
        self.set_freereg(base);
        self.last_line = line;
        self.emit(Inst::iabc(Op::Concat, base, nargs, 0, false));
        Ok(Exp::Reg(base))
    }

    /// True when `name` resolves, in the current function, to a named vararg
    /// kept virtual (so `name[k]` indexes the stack varargs via OP_VARGIDX).
    pub(super) fn vararg_virtual_local(&self, name: &str) -> bool {
        let lvl = self.lr();
        // a more-recent `global name` marker shadows the local
        if let Some(av) = lvl.avars.iter().rev().find(|a| a.name == Some(name))
            && av.global
        {
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
