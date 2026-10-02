//! Table reads and writes with a constant key: luna's field opcodes take
//! only string constants (see `Lowering::is_kstr`), anything else goes
//! through a register.

use super::lower::{Lowering, RK_BIT, enc_abc};
use crate::vm::isa::{self, Op};

impl Lowering {
    /// `R[dst] := R[t][K[k]]`.
    pub(super) fn get_field(&mut self, dst: u32, t: u32, k: u32) -> Result<(), String> {
        if k <= isa::MAX_C && self.is_kstr(k) {
            self.emit(enc_abc(Op::GetField, dst, t, k, false)?);
        } else {
            let key = self.k_in_temp(k)?;
            self.emit(enc_abc(Op::GetTable, dst, t, key, false)?);
        }
        Ok(())
    }

    /// `R[t][K[k]] := R[v]`.
    pub(super) fn set_field(&mut self, t: u32, k: u32, v: u32) -> Result<(), String> {
        if k <= isa::MAX_B && self.is_kstr(k) {
            self.emit(enc_abc(Op::SetField, t, k, v, false)?);
        } else {
            let key = self.k_in_temp(k)?;
            self.emit(enc_abc(Op::SetTable, t, key, v, false)?);
        }
        Ok(())
    }

    /// `R[dst] := Upvalue[up][K[k]]`. luna reserves `GetTabUp` for reads of
    /// the global environment and names the upvalue in an error only when
    /// the table was fetched into a register first, as its own compiler
    /// does for any other upvalue.
    pub(super) fn get_tabup(&mut self, dst: u32, up: u32, k: u32, env: bool) -> Result<(), String> {
        if env && k <= isa::MAX_C && self.is_kstr(k) {
            self.emit(enc_abc(Op::GetTabUp, dst, up, k, false)?);
        } else {
            let t = self.temp()?;
            self.emit(enc_abc(Op::GetUpval, t, up, 0, false)?);
            self.get_field(dst, t, k)?;
        }
        Ok(())
    }

    /// `Upvalue[up][K[k]] := R[v]`; `env` as for [`Self::get_tabup`].
    pub(super) fn set_tabup(&mut self, up: u32, k: u32, v: u32, env: bool) -> Result<(), String> {
        if env && k <= isa::MAX_B && self.is_kstr(k) {
            self.emit(enc_abc(Op::SetTabUp, up, k, v, false)?);
        } else {
            let t = self.temp()?;
            self.emit(enc_abc(Op::GetUpval, t, up, 0, false)?);
            self.set_field(t, k, v)?;
        }
        Ok(())
    }

    /// `R[dst] := R[t][RK(key)]`.
    pub(super) fn get_table_rk(&mut self, dst: u32, t: u32, key: u32) -> Result<(), String> {
        if key & RK_BIT != 0 {
            self.get_field(dst, t, key & 0xFF)
        } else {
            let key = self.r(key)?;
            self.emit(enc_abc(Op::GetTable, dst, t, key, false)?);
            Ok(())
        }
    }

    /// `R[t][RK(key)] := RK(val)`.
    pub(super) fn set_table_rk(&mut self, t: u32, key: u32, val: u32) -> Result<(), String> {
        let v = self.rk(val)?;
        if key & RK_BIT != 0 {
            self.set_field(t, key & 0xFF, v)
        } else {
            let key = self.r(key)?;
            self.emit(enc_abc(Op::SetTable, t, key, v, false)?);
            Ok(())
        }
    }

    /// `R[a+1] := R[b]; R[a] := R[b][RK(key)]`.
    pub(super) fn self_rk(&mut self, a: u32, b: u32, key: u32) -> Result<(), String> {
        let a = self.run(a, 2)?;
        let b = self.r(b)?;
        if key & RK_BIT != 0 {
            self.self_k(a, b, key & 0xFF)
        } else {
            let key = self.r(key)?;
            self.emit(enc_abc(Op::SelfOp, a, b, key, false)?);
            Ok(())
        }
    }

    /// `R[a+1] := R[b]; R[a] := R[b][K[k]]` with `a` and `b` already
    /// mapped; a key that is not a string goes through a register.
    pub(super) fn self_k(&mut self, a: u32, b: u32, k: u32) -> Result<(), String> {
        if self.is_kstr(k) {
            self.emit(enc_abc(Op::SelfOp, a, b, k, true)?);
        } else {
            let key = self.k_in_temp(k)?;
            self.emit(enc_abc(Op::SelfOp, a, b, key, false)?);
        }
        Ok(())
    }
}
