//! What an operand of each kind must name: a register in the frame, a
//! constant, a string constant, an upvalue.

use super::Checker;

impl Checker<'_> {
    /// Registers `first .. first + len` must lie below `max_stack`.
    pub(super) fn regs(&self, pc: usize, first: u32, len: u32) -> Result<(), String> {
        if first + len > self.max() {
            let what = if len <= 1 {
                format!("register {first}")
            } else {
                format!("registers {first}..{}", first + len - 1)
            };
            return Err(self.err(
                pc,
                format!("{what} out of range (stack size {})", self.max()),
            ));
        }
        Ok(())
    }

    pub(super) fn reg(&self, pc: usize, r: u32) -> Result<(), String> {
        self.regs(pc, r, 1)
    }

    pub(super) fn konst(&self, pc: usize, k: u32) -> Result<(), String> {
        let n = self.p.consts.len();
        if k as usize >= n {
            return Err(self.err(pc, format!("constant {k} out of range ({n} constants)")));
        }
        Ok(())
    }

    /// A constant the interpreter reads as a string key without checking
    /// (`GetField`, `SetField`, `GetTabUp`, `SetTabUp`, a `k` `SelfOp`).
    pub(super) fn kstr(&self, pc: usize, k: u32) -> Result<(), String> {
        self.konst(pc, k)?;
        if !matches!(self.p.consts[k as usize], crate::runtime::Value::Str(_)) {
            return Err(self.err(pc, format!("constant {k} is not a string")));
        }
        Ok(())
    }

    /// The value of a table write: constant `c` with `k` set, else a
    /// register.
    pub(super) fn value(&self, pc: usize, k: bool, c: u32) -> Result<(), String> {
        if k {
            self.konst(pc, c)
        } else {
            self.reg(pc, c)
        }
    }

    pub(super) fn upval(&self, pc: usize, u: u32) -> Result<(), String> {
        let n = self.p.upvals.len();
        if u as usize >= n {
            return Err(self.err(pc, format!("upvalue {u} out of range ({n} upvalues)")));
        }
        Ok(())
    }
}
