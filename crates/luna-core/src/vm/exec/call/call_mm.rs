//! Calling a value through its `__call` metamethod.

use super::*;

impl Vm {
    /// One `__call` hop of [`Vm::begin_call`] (PUC tryfuncTM): put the
    /// handler of `v` below the callee and its arguments. Out of line: it is
    /// rare, and keeping it inside makes the common call path pay for the
    /// registers it needs.
    #[cold]
    #[inline(never)]
    pub(super) fn shift_in_call_mm(
        &mut self,
        v: Value,
        func_slot: u32,
        nargs: u32,
        chain: u32,
    ) -> Result<(), LuaError> {
        let mm = self.get_mm(v, Mm::Call);
        if mm.is_nil() || self.call_mm_unusable(mm) {
            return Err(self.call_err_at(v, func_slot + 1 + nargs));
        }
        // PUC 5.5 dropped the chain cap from `MAXTAGRECUR = 200`
        // (the value 5.4's `lvm.c` uses) down to `MAXCCMT = 16`,
        // and the 5.5 test exercises the new tight bound directly
        // (calls.lua :225 builds a 16-deep chain and expects the
        // 16th to error). 5.4 calls.lua :194 instead builds a 20-
        // deep chain and expects it to succeed.
        let cap = if self.version >= crate::version::LuaVersion::Lua55 {
            15
        } else {
            MAX_CCMT
        };
        if chain > cap {
            return Err(self.rt_err("'__call' chain too long"));
        }
        // the callee and its arguments (and anything up to top)
        // shift up by one (PUC tryfuncTM); slots above them are
        // dead temps, and inserting into the whole stack would
        // move all of them and grow it on every hop
        let from = func_slot as usize;
        let end = (func_slot + 1 + nargs).max(self.top) as usize;
        self.grow_stack(end + 1)?;
        self.stack.copy_within(from..end, from + 1);
        self.stack[from] = mm;
        if self.top > func_slot {
            self.top += 1;
        }
        Ok(())
    }

    /// Up to 5.3 `tryfuncTM` takes one `__call` hop and needs a function
    /// there; anything else is a call error on the original object. 5.4
    /// retries the call with whatever `__call` holds, so chains resolve.
    pub(in crate::vm::exec) fn call_mm_unusable(&self, mm: Value) -> bool {
        self.version <= LuaVersion::Lua53 && !matches!(mm, Value::Closure(_) | Value::Native(_))
    }
}
