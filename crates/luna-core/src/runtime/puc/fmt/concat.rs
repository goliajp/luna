//! PUC's `luaV_concat` as far as what it makes goes.

use super::*;

impl Heap {
    /// PUC's `tostring` of a concatenation's operand: a number becomes the
    /// string of it.
    pub(in crate::runtime::puc) fn tostring_value(&mut self, v: &mut Value) -> Result<(), Oom> {
        let n = match *v {
            Value::Int(i) => Num::Int(i),
            Value::Float(f) => Num::Float(f),
            _ => return Ok(()),
        };
        let t = self.number_text(n);
        *v = Value::Str(self.try_intern(t.as_bytes())?);
        Ok(())
    }
}
