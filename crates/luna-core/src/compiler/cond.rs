//! Values with jump lists (PUC `expdesc` `t` / `f`): `and`, `or`, `not`
//! and comparisons, and the tests that go into the lists (`lcode.c`
//! `luaK_goiftrue`, `luaK_goiffalse`, `jumponcond`, `codenot`).

use super::*;

impl Compiler<'_> {
    /// The value of `e` and its true and false lists.
    pub(super) fn exp_parts(&self, e: Exp) -> (Exp, i32, i32) {
        match e {
            Exp::Jumps(i) => {
                let j = self.lr().jexps[i as usize];
                (j.e, j.t, j.f)
            }
            e => (e, NO_JUMP, NO_JUMP),
        }
    }

    /// `e` with the lists `t` and `f`.
    pub(super) fn exp_with(&mut self, e: Exp, t: i32, f: i32) -> Exp {
        debug_assert!(!matches!(e, Exp::Jumps(_)));
        if t == NO_JUMP && f == NO_JUMP {
            return e;
        }
        let l = self.l();
        l.jexps.push_or_abort(level::JExp { e, t, f });
        Exp::Jumps(l.jexps.len() as u32 - 1)
    }

    /// The registers the active locals take (PUC `luaY_nvarstack`).
    pub(super) fn nvarstack(&self) -> u32 {
        self.lr()
            .locals
            .iter()
            .rev()
            .find(|l| l.konst.is_none())
            .map_or(0, |l| l.reg + 1)
    }

    /// PUC `freereg` of a value's register: a temporary on top is free
    /// again.
    pub(super) fn free_reg(&mut self, r: u32) {
        if r >= self.nvarstack() && r + 1 == self.lr().freereg {
            self.l().freereg = r;
        }
    }

    /// PUC `negatecondition`: the test of the jump at `pc` the other way.
    fn negate_condition(&mut self, pc: usize) {
        let i = self.lr().code[pc - 1];
        debug_assert!(i.op().is_test() && !matches!(i.op(), Op::Test | Op::TestSet));
        self.l().code[pc - 1] = i.with_k(!i.k());
    }

    /// PUC `condjump`: a test and the jump it controls.
    pub(super) fn cond_jump(&mut self, i: Inst) -> Result<i32, SyntaxError> {
        self.emit(i);
        self.jump()
    }

    /// PUC `jumponcond`: a jump taken when `e` is `cond`.
    fn jump_on_cond(&mut self, e: Exp, cond: bool) -> Result<i32, SyntaxError> {
        if let Exp::Reloc(pc) = e {
            let i = self.lr().code[pc];
            if i.op() == Op::Not && pc + 1 == self.here() {
                // the `not` is taken back and its operand tested the other way
                let l = self.l();
                l.code.pop();
                l.lines.pop();
                return self.cond_jump(Inst::iabc(Op::Test, i.b(), 0, 0, !cond));
            }
        }
        let r = self.exp_to_anyreg_value(e)?;
        self.free_reg(r);
        self.cond_jump(Inst::iabc(Op::TestSet, 0xFF, r, 0, cond))
    }

    /// PUC `luaK_goiftrue`: go on when `e` is true, its false list jumping.
    pub(super) fn go_if_true(&mut self, e: Exp) -> Result<Exp, SyntaxError> {
        let (v, t, mut f) = self.exp_parts(e);
        let pc = match v {
            Exp::Jmp(pc) => {
                self.negate_condition(pc);
                pc as i32
            }
            Exp::Const(_) | Exp::Int(_) | Exp::Float(_) | Exp::True => NO_JUMP,
            _ => self.jump_on_cond(v, false)?,
        };
        self.concat_list(&mut f, pc)?;
        self.patch_to_here(t)?;
        Ok(self.exp_with(v, NO_JUMP, f))
    }

    /// PUC `luaK_goiffalse`: go on when `e` is false, its true list jumping.
    pub(super) fn go_if_false(&mut self, e: Exp) -> Result<Exp, SyntaxError> {
        let (v, mut t, f) = self.exp_parts(e);
        let pc = match v {
            Exp::Jmp(pc) => pc as i32,
            Exp::Nil | Exp::False => NO_JUMP,
            _ => self.jump_on_cond(v, true)?,
        };
        self.concat_list(&mut t, pc)?;
        self.patch_to_here(f)?;
        Ok(self.exp_with(v, t, NO_JUMP))
    }

    /// PUC `codenot`.
    pub(super) fn code_not(&mut self, e: Exp) -> Result<Exp, SyntaxError> {
        let (v, t, f) = self.exp_parts(e);
        let v = match v {
            Exp::Nil | Exp::False => Exp::True,
            Exp::Const(_) | Exp::Int(_) | Exp::Float(_) | Exp::True => Exp::False,
            Exp::Jmp(pc) => {
                self.negate_condition(pc);
                v
            }
            v => {
                let r = self.exp_to_anyreg_value(v)?;
                self.free_reg(r);
                Exp::Reloc(self.emit(Inst::iabc(Op::Not, 0, r, 0, false)))
            }
        };
        // the lists change places, and their values are useless now
        self.remove_values(t);
        self.remove_values(f);
        Ok(self.exp_with(v, f, t))
    }

    /// PUC `cond`: a statement's condition, compiled to the jumps taken
    /// when it is false.
    pub(super) fn cond(&mut self, id: ExprId) -> Result<i32, SyntaxError> {
        let e = self.expr(id)?;
        let e = self.cond_of(e)?;
        Ok(self.exp_parts(e).2)
    }

    /// [`Self::cond`] of the compiled condition `e`: nil is false there.
    pub(super) fn cond_of(&mut self, e: Exp) -> Result<Exp, SyntaxError> {
        let e = match e {
            Exp::Nil => Exp::False,
            e => e,
        };
        self.go_if_true(e)
    }
}
