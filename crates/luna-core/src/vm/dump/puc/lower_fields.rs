//! Table reads and writes with a constant key: a string key takes the
//! field opcodes (which read it as a string without looking, see
//! `Lowering::is_kstr`), any other constant the `K` forms.

use super::lower::{Lowering, Rk, enc_abc};
use crate::vm::isa::{self, Op};

impl Lowering {
    /// `R[dst] := R[t][K[k]]`.
    pub(super) fn get_field(&mut self, dst: u32, t: u32, k: u32) -> Result<(), String> {
        let op = if self.is_kstr(k) {
            Op::GetField
        } else {
            Op::GetTableK
        };
        let k = self.byte(k, "constant key")?;
        self.emit(enc_abc(op, dst, t, k, false)?);
        Ok(())
    }

    /// `R[t][K[k]] := v`, the value `(C, k)`.
    pub(super) fn set_field(&mut self, t: u32, k: u32, v: (u32, bool)) -> Result<(), String> {
        let op = if self.is_kstr(k) {
            Op::SetField
        } else {
            Op::SetTableK
        };
        let k = self.byte(k, "constant key")?;
        self.emit(enc_abc(op, t, k, v.0, v.1)?);
        Ok(())
    }

    /// `R[dst] := Upvalue[up][K[k]]`.
    pub(super) fn get_tabup(&mut self, dst: u32, up: u32, k: u32) -> Result<(), String> {
        let k = self.byte(k, "constant key")?;
        let inst = if self.is_kstr(k) {
            enc_abc(Op::GetTabUp, dst, up, k, false)?
        } else {
            enc_abc(Op::GetTabUpR, dst, up, k, true)?
        };
        self.emit(inst);
        Ok(())
    }

    /// `Upvalue[up][K[k]] := v`.
    pub(super) fn set_tabup(&mut self, up: u32, k: u32, v: (u32, bool)) -> Result<(), String> {
        let op = if self.is_kstr(k) {
            Op::SetTabUp
        } else {
            Op::SetTabUpK
        };
        let k = self.byte(k, "constant key")?;
        self.emit(enc_abc(op, up, k, v.0, v.1)?);
        Ok(())
    }

    /// `R[dst] := R[t][RK(key)]`.
    pub(super) fn get_table_rk(&mut self, dst: u32, t: u32, key: u32) -> Result<(), String> {
        match self.rk(key)? {
            Rk::K(k) => self.get_field(dst, t, k),
            Rk::R(r) => {
                self.emit(enc_abc(Op::GetTable, dst, t, r, false)?);
                Ok(())
            }
        }
    }

    /// `R[t][RK(key)] := RK(val)`.
    pub(super) fn set_table_rk(&mut self, t: u32, key: u32, val: u32) -> Result<(), String> {
        let v = self.rk_value(val)?;
        match self.rk(key)? {
            Rk::K(k) => self.set_field(t, k, v),
            Rk::R(r) => {
                self.emit(enc_abc(Op::SetTable, t, r, v.0, v.1)?);
                Ok(())
            }
        }
    }

    /// `R[a+1] := R[b]; R[a] := R[b][RK(key)]`.
    pub(super) fn self_rk(&mut self, a: u32, b: u32, key: u32) -> Result<(), String> {
        let a = self.run(a, 2)?;
        let b = self.r(b)?;
        match self.rk(key)? {
            Rk::K(k) => self.self_k(a, b, k),
            Rk::R(r) => {
                self.emit(enc_abc(Op::SelfOp, a, b, r, false)?);
                Ok(())
            }
        }
    }

    /// `R[a+1] := R[b]; R[a] := R[b][K[k]]` with `a` and `b` already
    /// mapped. A method name is a string; a crafted chunk's other constant
    /// goes through a scratch register.
    pub(super) fn self_k(&mut self, a: u32, b: u32, k: u32) -> Result<(), String> {
        if self.is_kstr(k) && k <= isa::MAX_C {
            self.emit(enc_abc(Op::SelfOp, a, b, k, true)?);
        } else {
            let key = self.k_in_temp(k)?;
            self.emit(enc_abc(Op::SelfOp, a, b, key, false)?);
        }
        Ok(())
    }

    /// 5.1 `GETGLOBAL`: `R[dst] := _ENV[K[k]]`; a name past constant 255
    /// goes through registers, as luna's compiler reads it.
    pub(super) fn get_global(&mut self, dst: u32, k: u32) -> Result<(), String> {
        if k <= isa::MAX_C && self.is_kstr(k) {
            return self.get_tabup(dst, 0, k);
        }
        let (t, key) = (self.temp()?, self.temp()?);
        self.emit(enc_abc(Op::GetUpval, t, 0, 0, false)?);
        self.load_k(key, k)?;
        self.emit(enc_abc(Op::GetTable, dst, t, key, false)?);
        Ok(())
    }

    /// 5.1 `SETGLOBAL`: `_ENV[K[k]] := R[v]`, as [`Self::get_global`].
    pub(super) fn set_global(&mut self, v: u32, k: u32) -> Result<(), String> {
        if k <= isa::MAX_B && self.is_kstr(k) {
            return self.set_tabup(0, k, (v, false));
        }
        let (t, key) = (self.temp()?, self.temp()?);
        self.emit(enc_abc(Op::GetUpval, t, 0, 0, false)?);
        self.load_k(key, k)?;
        self.emit(enc_abc(Op::SetTable, t, key, v, false)?);
        Ok(())
    }
}
