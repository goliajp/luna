//! Discharging an `Exp` into registers.

use super::*;

impl Compiler<'_> {
    /// PUC `exp2reg`: the value of `e`, and of its jump lists, in `reg`.
    pub(super) fn exp_to_reg(&mut self, e: Exp, reg: u32) -> Result<(), SyntaxError> {
        let (v, mut t, f) = self.exp_parts(e);
        self.discharge_to_reg(v, reg)?;
        if let Exp::Jmp(pc) = v {
            self.concat_list(&mut t, pc as i32)?;
        }
        if t == NO_JUMP && f == NO_JUMP {
            return Ok(());
        }
        let (mut p_f, mut p_t) = (None, None);
        if self.need_value(t) || self.need_value(f) {
            let fj = if matches!(v, Exp::Jmp(_)) {
                NO_JUMP
            } else {
                self.jump()?
            };
            p_f = Some(self.code_loadbool(reg, Op::LFalseSkip));
            p_t = Some(self.code_loadbool(reg, Op::LoadTrue));
            self.patch_to_here(fj)?;
        }
        let end = self.get_label();
        self.patch_list_aux(f, end, Some(reg), p_f.unwrap_or(end))?;
        self.patch_list_aux(t, end, Some(reg), p_t.unwrap_or(end))
    }

    /// PUC `code_loadbool`.
    fn code_loadbool(&mut self, reg: u32, op: Op) -> usize {
        self.get_label();
        self.emit(Inst::iabc(op, reg, 0, 0, false))
    }

    /// PUC `discharge2anyreg`: the value of `e` (not its lists) in a
    /// register.
    pub(super) fn exp_to_anyreg_value(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        match e {
            Exp::Reg(r) => Ok(r),
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Ok(base)
            }
            e => {
                let r = self.reserve(1)?;
                self.discharge_to_reg(e, r)?;
                Ok(r)
            }
        }
    }

    /// PUC `discharge2reg`: the value of `e` (not its lists) in `reg`.
    fn discharge_to_reg(&mut self, e: Exp, reg: u32) -> Result<(), SyntaxError> {
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
            Exp::Jmp(_) => {}
            Exp::Jumps(_) => unreachable!("a value without its lists"),
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
        // PUC `luaK_exp2nextreg` frees the value's own temporary first
        if let Exp::Jumps(_) = e {
            let (v, t, f) = self.exp_parts(e);
            let v = self.discharge_vars(v);
            if let Exp::Reg(r) = v {
                self.free_reg(r);
            }
            let e = self.exp_with(v, t, f);
            let reg = self.reserve(1)?;
            self.exp_to_reg(e, reg)?;
            return Ok(reg);
        }
        let reg = self.reserve(1)?;
        self.exp_to_reg(e, reg)?;
        Ok(reg)
    }

    /// PUC `luaK_exp2val`: a test or a value with jump lists in a register.
    pub(super) fn exp_to_val(&mut self, e: Exp) -> Result<Exp, SyntaxError> {
        Ok(match e {
            Exp::Jmp(_) | Exp::Jumps(_) => Exp::Reg(self.exp_to_anyreg(e)?),
            e => e,
        })
    }

    /// PUC `luaK_dischargevars` of a call or `...`: its one value.
    pub(super) fn discharge_vars(&mut self, e: Exp) -> Exp {
        match e {
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Exp::Reg(base)
            }
            e => e,
        }
    }

    pub(super) fn exp_to_anyreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        match e {
            Exp::Reg(r) => Ok(r),
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Ok(base)
            }
            Exp::Jumps(_) => {
                // a temporary takes the values of the lists itself; a local
                // cannot
                let (v, t, f) = self.exp_parts(e);
                let v = self.discharge_vars(v);
                if let Exp::Reg(r) = v
                    && r >= self.nvarstack()
                {
                    let e = self.exp_with(v, t, f);
                    self.exp_to_reg(e, r)?;
                    return Ok(r);
                }
                let e = self.exp_with(v, t, f);
                self.exp_to_nextreg(e)
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
