//! Conditions of `if` and `while`: compiled straight to conditional jumps.
//!
//! `and`, `or`, `not` and parentheses never produce a value here; each leaf
//! becomes one test and one jump, the way PUC's `luaK_goiftrue` /
//! `luaK_goiffalse` walk an expression's true and false jump lists. Compiling
//! `a < b and c < d` as a value first would materialize the right comparison
//! into a register (`LFALSESKIP` / `LOADTRUE`) and then `TEST` it.

use super::*;

/// What the condition's last test was, for the caller's line attribution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LastTest {
    /// a comparison: emitted where the comparison was compiled
    Cmp,
    /// a `TEST` of a value, emitted after the whole condition was read
    Test,
    /// a constant that never takes the jump: nothing was emitted
    None,
}

impl Compiler<'_> {
    /// Compile a condition; the returned jumps are taken when it is FALSE.
    pub(super) fn cond_jump_false(&mut self, id: ExprId) -> Result<(Jumps, LastTest), SyntaxError> {
        let mut jumps = Jumps::new(self.heap.mem());
        let last = self.cond_jumps(id, false, &mut jumps)?;
        Ok((jumps, last))
    }

    /// Emit the tests of `id`, collecting into `out` the jumps taken when its
    /// truth value is `jump_if`; paths with the other value fall through.
    fn cond_jumps(
        &mut self,
        id: ExprId,
        jump_if: bool,
        out: &mut Jumps,
    ) -> Result<LastTest, SyntaxError> {
        match *self.ast.expr(id) {
            Expr::BinOp {
                op: op @ (BinOp::And | BinOp::Or),
                lhs,
                rhs,
                line,
            } => {
                // the left operand decides the whole when it is false (`and`)
                // or true (`or`)
                let decides = op == BinOp::Or;
                self.last_line = line;
                if jump_if == decides {
                    self.cond_leaf_or_tree(lhs, decides, out, line)?;
                    self.cond_jumps(rhs, jump_if, out)
                } else {
                    let mut past = Jumps::new(self.heap.mem());
                    self.cond_leaf_or_tree(lhs, decides, &mut past, line)?;
                    let last = self.cond_jumps(rhs, jump_if, out)?;
                    for pc in past.iter() {
                        self.patch_to_here(pc)?;
                    }
                    Ok(last)
                }
            }
            Expr::UnOp {
                op: UnOp::Not,
                operand,
                ..
            } => self.cond_jumps(operand, !jump_if, out),
            Expr::Paren(inner) => self.cond_jumps(inner, jump_if, out),
            // `a ~= b` is `a == b` tested the other way round (as a value it
            // is materialized, see `negate_cmp`)
            Expr::BinOp {
                op: BinOp::Ne,
                lhs,
                rhs,
                line,
            } => {
                let saved = self.lr().freereg;
                let e = self.binop(BinOp::Eq, lhs, rhs, line)?;
                self.cond_leaf(e, saved, !jump_if, out)
            }
            _ => {
                let saved = self.lr().freereg;
                let e = self.expr(id)?;
                self.cond_leaf(e, saved, jump_if, out)
            }
        }
    }

    /// The left operand of `and` / `or`. PUC tests it while reading the
    /// operator, so a test emitted for a plain value takes the operator's line.
    fn cond_leaf_or_tree(
        &mut self,
        id: ExprId,
        jump_if: bool,
        out: &mut Jumps,
        op_line: u32,
    ) -> Result<(), SyntaxError> {
        if self.cond_jumps(id, jump_if, out)? == LastTest::Test {
            let jmp = self.here() - 1;
            self.l().lines[jmp - 1] = op_line;
            self.l().lines[jmp] = op_line;
        }
        Ok(())
    }

    /// Test the compiled operand `e`; `saved` is the free register from
    /// before it was compiled.
    fn cond_leaf(
        &mut self,
        e: Exp,
        saved: u32,
        jump_if: bool,
        out: &mut Jumps,
    ) -> Result<LastTest, SyntaxError> {
        let last = match e {
            Exp::Cmp { op, l, r, c } => {
                self.emit(Inst::iabc(op, l, r, c, jump_if))?;
                LastTest::Cmp
            }
            // PUC `luaK_goiftrue` / `luaK_goiffalse`: a constant whose truth
            // value never takes the jump emits nothing
            Exp::True | Exp::Int(_) | Exp::Float(_) | Exp::Const(_) if !jump_if => {
                self.set_freereg(saved);
                return Ok(LastTest::None);
            }
            Exp::Nil | Exp::False if jump_if => {
                self.set_freereg(saved);
                return Ok(LastTest::None);
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, jump_if))?;
                LastTest::Test
            }
        };
        self.set_freereg(saved);
        out.push(self.emit_jump()?)?;
        Ok(last)
    }
}
