//! Discharging an `Exp` into registers.

use super::*;

impl Compiler<'_> {
    /// Materialize into a specific register.
    pub(super) fn exp_to_reg(&mut self, e: Exp, reg: u32) -> Result<(), SyntaxError> {
        match e {
            Exp::Nil => {
                // PUC 5.1 `luaK_nil`: at function start a register above the
                // active variables is nil already
                let fresh = || {
                    let lvl = self.lr();
                    lvl.code.is_empty()
                        && lvl.locals.iter().all(|v| v.konst.is_some() || v.reg < reg)
                };
                if !(self.version == LuaVersion::Lua51 && fresh()) {
                    self.emit(Inst::iabc(Op::LoadNil, reg, 0, 0, false));
                }
            }
            Exp::True => {
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
            }
            Exp::False => {
                self.emit(Inst::iabc(Op::LoadFalse, reg, 0, 0, false));
            }
            Exp::Int(i) => {
                if (-65535..=65535).contains(&i) {
                    self.number_const_before_54(Value::Int(i));
                    self.emit(Inst::iasbx(Op::LoadI, reg, i as i32));
                } else {
                    let c = self.const_idx(Value::Int(i));
                    self.load_const(reg, c);
                }
            }
            Exp::Float(mut f) => {
                if f == 0.0 && self.version == LuaVersion::Lua51 {
                    f = *self.l().zero_51.get_or_insert(f);
                }
                let as_int = f as i32;
                // bit-compare so the LoadF fast path doesn't fold -0.0 to +0.0
                // (`-0.0 == 0.0` but their bit patterns differ)
                if (-65535..=65535).contains(&as_int) && (as_int as f64).to_bits() == f.to_bits() {
                    self.number_const_before_54(Value::Float(f));
                    self.emit(Inst::iasbx(Op::LoadF, reg, as_int));
                } else {
                    let c = self.const_idx(Value::Float(f));
                    self.load_const(reg, c);
                }
            }
            Exp::Const(c) => self.load_const(reg, c),
            Exp::Reg(r) => {
                if r != reg {
                    self.emit(Inst::iabc(Op::Move, reg, r, 0, false));
                }
            }
            Exp::Reloc(pc) => self.patch_dest(pc, reg),
            Exp::Cmp { op, l, r, c } => {
                self.emit(Inst::iabc(op, l, r, c, true));
                self.emit(Inst::isj(Op::Jmp, 1));
                self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                let tpad = self.here();
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                // Jmp(1) above skips the LFalseSkip and lands on the LoadTrue
                // pad — that pc is a jump destination.
                self.mark_target(tpad);
            }
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                if base != reg {
                    self.emit(Inst::iabc(Op::Move, reg, base, 0, false));
                }
            }
        }
        Ok(())
    }

    pub(super) fn exp_to_nextreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        let reg = self.reserve(1)?;
        self.exp_to_reg(e, reg)?;
        Ok(reg)
    }

    pub(super) fn exp_to_anyreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        match e {
            Exp::Reg(r) => Ok(r),
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Ok(base)
            }
            e => self.exp_to_nextreg(e),
        }
    }

    /// Before 5.4 every number is loaded from the constant table (there is
    /// no `LOADI` / `LOADF`): the constant enters the table where PUC's
    /// code generator adds it, so a dump lists constants in PUC's order.
    pub(super) fn number_const_before_54(&mut self, v: Value) {
        if self.version < LuaVersion::Lua54 {
            self.const_idx(v);
        }
    }

    /// 5.1: zeros of a condition the parser folded away still entered the
    /// constant table (see `Level::zero_51`).
    pub(super) fn note_zeros(&mut self, zeros: &[f64]) {
        if let Some(&z) = zeros.first() {
            self.l().zero_51.get_or_insert(z);
        }
    }
}
