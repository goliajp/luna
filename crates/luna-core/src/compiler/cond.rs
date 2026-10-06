//! Conditions of `if` and `while`: compiled straight to conditional jumps.
//!
//! `and`, `or`, `not` and parentheses never produce a value here; each leaf
//! becomes one test and one jump, the way PUC's `luaK_goiftrue` /
//! `luaK_goiffalse` walk an expression's true and false jump lists. Compiling
//! `a < b and c < d` as a value first would materialize the right comparison
//! into a register (`LFALSESKIP` / `LOADTRUE`) and then `TEST` it.

use super::*;
use crate::runtime::mem::LVec;

/// An `and` / `or` on the left spine of a condition, whose left operand
/// is being tested.
struct AndOrLevel {
    rhs: ExprId,
    line: u32,
    /// the jump wanted of the whole operation
    jump_if: bool,
    /// the left operand's jumps when they are not the jump wanted: patched
    /// to after the right operand
    past: Option<Jumps>,
    /// the enclosing level whose `past` the operation's jumps go to;
    /// `None` for the condition's own list
    target: Option<usize>,
}

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
    /// truth value is `jump_if`; paths with the other value fall through. A
    /// chain of `and` / `or` is walked without recursion along its left
    /// operands, as PUC's parser reads it.
    fn cond_jumps(
        &mut self,
        id: ExprId,
        jump_if: bool,
        out: &mut Jumps,
    ) -> Result<LastTest, SyntaxError> {
        if crate::native_stack::is_low(crate::native_stack::RESERVE) {
            return Err(self.too_deep());
        }
        let mem = self.heap.mem();
        // the `and` / `or` nodes down the left spine, outermost first
        let mut levels: LVec<AndOrLevel> = LVec::new(mem);
        let mut cur = id;
        let mut cur_jump_if = jump_if;
        while let Expr::BinOp {
            op: op @ (BinOp::And | BinOp::Or),
            lhs,
            rhs,
            line,
        } = *self.ast.expr(cur)
        {
            // the left operand decides the whole when it is false (`and`)
            // or true (`or`): tested into `out` when that is the jump wanted,
            // else into a list patched past the right operand
            let decides = op == BinOp::Or;
            self.last_line = line;
            let past = (cur_jump_if != decides).then(|| Jumps::new(mem));
            let target = match levels.last() {
                Some(l) if l.past.is_some() => Some(levels.len() - 1),
                Some(l) => l.target,
                None => None,
            };
            levels.push_or_abort(AndOrLevel {
                rhs,
                line,
                jump_if: cur_jump_if,
                past,
                target,
            });
            cur = lhs;
            cur_jump_if = decides;
        }
        let n = levels.len();
        let mut last = match levels.last_mut() {
            Some(l) if l.past.is_some() => {
                let past = l.past.as_mut().expect("checked");
                self.cond_jumps_plain(cur, cur_jump_if, past)?
            }
            Some(l) => match l.target {
                Some(t) => {
                    let past = levels[t].past.as_mut().expect("a level with a list");
                    self.cond_jumps_plain(cur, cur_jump_if, past)?
                }
                None => self.cond_jumps_plain(cur, cur_jump_if, out)?,
            },
            None => self.cond_jumps_plain(cur, cur_jump_if, out)?,
        };
        for _ in 0..n {
            let level = levels.pop().expect("a level per iteration");
            // a plain value's test (the left operand just tested) takes the
            // operator's line, as PUC tests it while reading the operator
            if last == LastTest::Test {
                let jmp = self.here() - 1;
                self.l().lines[jmp - 1] = level.line;
                self.l().lines[jmp] = level.line;
            }
            last = match level.target {
                Some(t) => {
                    let past = levels[t].past.as_mut().expect("a level with a list");
                    self.cond_jumps(level.rhs, level.jump_if, past)?
                }
                None => self.cond_jumps(level.rhs, level.jump_if, out)?,
            };
            if let Some(past) = level.past {
                for pc in past.iter() {
                    self.patch_to_here(pc)?;
                }
            }
        }
        Ok(last)
    }

    /// [`Self::cond_jumps`] of an expression that is not `and` / `or`.
    fn cond_jumps_plain(
        &mut self,
        id: ExprId,
        jump_if: bool,
        out: &mut Jumps,
    ) -> Result<LastTest, SyntaxError> {
        match *self.ast.expr(id) {
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
                self.emit(Inst::iabc(op, l, r, c, jump_if));
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
                self.emit(Inst::iabc(Op::Test, r, 0, 0, jump_if));
                LastTest::Test
            }
        };
        self.set_freereg(saved);
        out.push(self.emit_jump());
        Ok(last)
    }
}
