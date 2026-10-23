//! The upvalues of Lua functions as the C API's debug interface sees them
//! (`lua_getupvalue`, `lua_setupvalue`, `lua_upvalueid`, `lua_upvaluejoin`).

use super::*;
use crate::vm::lib_debug;

impl Vm {
    /// PUC `lua_getupvalue` of a Lua function: the name and value of
    /// upvalue `n`.
    pub(crate) fn host_upvalue(&self, f: Value, n: i64) -> Option<(String, Value)> {
        let Value::Closure(cl) = f else { return None };
        let idx = lib_debug::visible_upvalue_index(self, cl, n)?;
        let name = lib_debug::upvalue_name(self, cl, idx)?;
        Some((name, self.upvalue_value(cl, idx)))
    }

    /// PUC `lua_setupvalue` of a Lua function: store `v` in upvalue `n`
    /// and return its name.
    pub(crate) fn host_set_upvalue(&mut self, f: Value, n: i64, v: Value) -> Option<String> {
        let Value::Closure(cl) = f else { return None };
        let idx = lib_debug::visible_upvalue_index(self, cl, n)?;
        let name = lib_debug::upvalue_name(self, cl, idx)?;
        self.upvalue_set_value(cl, idx, v);
        Some(name)
    }

    /// PUC `lua_upvalueid` of a Lua function: the address of the cell of
    /// upvalue `n`.
    pub(crate) fn host_upvalue_id(&self, f: Value, n: i64) -> Option<*const ()> {
        let Value::Closure(cl) = f else { return None };
        let i = usize::try_from(n.checked_sub(1)?).ok()?;
        cl.upvals().get(i).map(|u| u.as_ptr() as *const ())
    }

    /// PUC `lua_upvaluejoin`: upvalue `n1` of `f1` refers to upvalue `n2`
    /// of `f2`. Both are Lua functions with those upvalues.
    pub(crate) fn host_upvalue_join(&mut self, f1: Value, n1: i64, f2: Value, n2: i64) {
        let (Value::Closure(f1), Value::Closure(f2)) = (f1, f2) else {
            return;
        };
        let (Ok(i1), Ok(i2)) = (usize::try_from(n1 - 1), usize::try_from(n2 - 1)) else {
            return;
        };
        let (Some(_), Some(&uv)) = (f1.upvals().get(i1), f2.upvals().get(i2)) else {
            return;
        };
        // SAFETY: `f1` is a closure the caller holds on its stack; `uv` is a
        // separate handle read out before the borrow, which covers one store
        unsafe { f1.as_mut() }.upvals_mut()[i1] = uv;
        self.heap.barrier_back(f1);
    }
}
