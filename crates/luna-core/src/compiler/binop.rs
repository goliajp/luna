//! Binary operators: operand evaluation order and comparisons (the
//! constant- and immediate-operand forms are in `binop_const`).

use super::binop_const::Operand;
use super::*;

/// What [`Compiler::binop_open`] set up before the left operand: the free
/// register and the forced line to put back.
#[derive(Clone, Copy)]
pub(super) struct BinOpOpen {
    pub(super) saved: u32,
    pub(super) saved_force: Option<u32>,
}

impl Compiler<'_> {
    /// A binary operation other than `and`, `or` and `..`, which have
    /// their own shapes. [`Compiler::expr`] compiles a chain of them without
    /// recursion by calling [`Self::binop_open`] and [`Self::binop_close`]
    /// itself.
    pub(super) fn binop(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        match op {
            BinOp::And | BinOp::Or => return self.and_or(op, lhs, rhs, line),
            BinOp::Concat => return self.concat(lhs, rhs, line),
            _ => {}
        }
        let (open, le) = self.binop_open(op, lhs, line)?;
        let le = match le {
            Some(le) => le,
            None => self.expr(lhs)?,
        };
        self.binop_close(op, le, rhs, line, open)
    }

    /// Before the left operand of `op` is compiled: the value it has
    /// without being compiled, when it has one.
    pub(super) fn binop_open(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        line: u32,
    ) -> Result<(BinOpOpen, Option<Exp>), SyntaxError> {
        debug_assert!(!matches!(op, BinOp::And | BinOp::Or | BinOp::Concat));
        let saved = self.lr().freereg;
        // PUC's `infix` discharges the left operand *after* consuming the
        // operator token (luaK_indexed → luaK_exp2anyreg called from infix),
        // so any GET emitted for the lhs lands on the operator's line, not
        // on the line of the lhs itself (db.lua :193 line-trace family).
        // luna parses the lhs ahead of time; pin the line through
        // `force_line` for the duration of the lhs walk so every emit it
        // performs is attributed to the operator's line, then drop the pin
        // before parsing the rhs (which discharges at its own last-token
        // line, matching PUC).
        let saved_force = self.force_line.replace(line);
        let open = BinOpOpen { saved, saved_force };
        // a 5.1 left operand whose logic folds away (see `numeral`)
        if self.version != LuaVersion::Lua51 || !is_logical(self.ast, lhs) {
            return Ok((open, None));
        }
        let mut zeros = Vec::new();
        let le = numeral(self.ast, lhs, self.version, &mut zeros).map(|n| {
            self.note_zeros(&zeros);
            match n {
                Num::Int(i) => Exp::Int(i),
                Num::Float(f) => Exp::Float(f),
            }
        });
        Ok((open, le))
    }

    /// The rest of `op`, its left operand compiled to `le`.
    pub(super) fn binop_close(
        &mut self,
        op: BinOp,
        le: Exp,
        rhs: ExprId,
        line: u32,
        open: BinOpOpen,
    ) -> Result<Exp, SyntaxError> {
        if self.version <= LuaVersion::Lua53 {
            return self.binop_close_classic(op, le, rhs, line, open);
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            return self.binop_eq_modern(op, le, rhs, open);
        }
        let BinOpOpen { saved, saved_force } = open;
        let mut zeros = Vec::new();
        if let Some(folded) = fold_arith(op, &le, self.ast, rhs, self.version, &mut zeros) {
            self.note_zeros(&zeros);
            self.force_line = saved_force;
            return Ok(folded);
        }
        // PUC 5.1 puts a numeral left operand in the constant table only
        // after the right operand (`luaK_infix` leaves numerals be, and
        // `codearith` takes the right one first). For a zero that order
        // decides which sign the function's zeros share (see `zero_51`).
        let deferred = match le {
            Exp::Float(f) if f == 0.0 && self.version == LuaVersion::Lua51 => Some(f),
            _ => None,
        };
        // A numeral on the left stays out of a register until the right
        // operand is known: it may become the instruction's own operand, and
        // otherwise it is loaded after the right one (PUC `luaK_infix`).
        let left_numeral = deferred.is_none() && matches!(le, Exp::Int(_) | Exp::Float(_));
        let mut l = if left_numeral {
            None
        } else {
            Some(match deferred {
                Some(_) => self.reserve(1)?,
                None => self.exp_to_anyreg(le)?,
            })
        };
        self.force_line = saved_force;
        // Protect the left operand's register if it is a fresh temporary at the
        // top of the stack (e.g. a CONCAT result): evaluating the right operand
        // must not reuse and clobber it before the binary op reads it.
        if let Some(l) = l
            && l >= saved
        {
            self.set_freereg(l + 1);
        }
        // `x - K` with K what PUC's parser folds to the constant 0 (`(0)`,
        // `1 - 1`, `a and nil or 0`...) is PUC's `ADDI x 0`, even when K's
        // code still runs: `SubI x 0`, after that code
        let sub_zero = op == BinOp::Sub && {
            let ast = self.ast;
            matches!(
                ct_operand(ast, rhs, &mut |name| self.ct_const_named(self.nm(name))),
                Some(CtConst::Int(0))
            )
        };
        let mut re = self.expr(rhs)?;
        if sub_zero && !matches!(re, Exp::Int(0)) {
            self.exp_to_anyreg(re)?;
            re = Exp::Int(0);
        }
        // The operand that goes into the instruction instead of a register,
        // and the side it was written on.
        let mut in_inst: Option<(Operand, bool)> = None;
        let mut right_reg = None;
        if l.is_none() {
            if matches!(re, Exp::Int(_) | Exp::Float(_) | Exp::Const(_)) {
                // Two numerals the fold left alone (`7.5 // 0`), as PUC's
                // `codearith` takes them: 5.4+ moves the left one of `+`, `*`
                // (and an integer of a bitwise operator) to the right; the
                // right one becomes the operand, its constant first, and the
                // left one a register; with no such form the right one takes
                // its register first.
                // ... and an immediate `I << x` is `SHLI`
                let swap = self.version >= LuaVersion::Lua54
                    && (matches!(op, BinOp::Add | BinOp::Mul)
                        || matches!(op, BinOp::BAnd | BinOp::BOr | BinOp::BXor)
                            && matches!(le, Exp::Int(_))
                        || op == BinOp::Shl);
                let saved_line = self.force_line.replace(line);
                if swap && let Some(form) = self.const_operand(op, &le, true) {
                    in_inst = Some((form, true));
                    right_reg = Some(self.exp_to_anyreg(re)?);
                } else if let Some(form) = self.const_operand(op, &re, false) {
                    in_inst = Some((form, false));
                    l = Some(self.exp_to_anyreg(le)?);
                } else {
                    let r = self.exp_to_anyreg(re)?;
                    if r >= saved {
                        self.set_freereg(r + 1);
                    }
                    right_reg = Some(r);
                    l = Some(self.exp_to_anyreg(le)?);
                }
                self.force_line = saved_line;
            } else if let Some(form) = self.const_operand(op, &le, true) {
                in_inst = Some((form, true));
            }
        }
        if l.is_some()
            && right_reg.is_none()
            && in_inst.is_none()
            && let Some(form) = self.const_operand(op, &re, false)
        {
            in_inst = Some((form, false));
        }
        // the register operand(s)
        let (l, r) = match (l, in_inst, right_reg) {
            // swapped: the register is the right operand's
            (_, Some((_, true)), Some(r)) => (r, 0),
            (Some(l), None, Some(r)) => (l, r),
            (Some(l), Some(_), _) => (l, 0),
            (Some(l), None, None) => (l, self.exp_to_anyreg(re)?),
            (None, _, _) => {
                let r = self.exp_to_anyreg(re)?;
                if in_inst.is_some() {
                    (r, 0)
                } else {
                    // the left numeral has no in-instruction form here: it
                    // goes above the right operand, which it must not clobber
                    if r >= saved {
                        self.set_freereg(r + 1);
                    }
                    let saved_line = self.force_line.replace(line);
                    let l = self.exp_to_anyreg(le)?;
                    self.force_line = saved_line;
                    (l, r)
                }
            }
        };
        if let Some(f) = deferred {
            let saved_line = self.force_line.replace(line);
            self.exp_to_reg(Exp::Float(f), l)?;
            self.force_line = saved_line;
        }
        self.set_freereg(saved);
        // PUC attributes the arith op itself to the operator's line, but
        // leaves `lastline` at the rhs's last token (so a following SETTABUP
        // / SETUPVAL for the assignment lands on the rhs's end line, not the
        // operator). Pin the line for the arith emit, but don't stomp
        // `last_line` permanently.
        let saved_force_arith = self.force_line.replace(line);
        let r_op = match in_inst {
            Some((form, flip)) => self.emit_const_operand(op, l, form, flip),
            None => self.emit_binop(op, l, r),
        };
        self.force_line = saved_force_arith;
        r_op
    }
}
