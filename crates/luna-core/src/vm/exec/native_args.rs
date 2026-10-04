//! Argument, upvalue and result access for running natives.

use super::*;

impl Vm {
    /// Read upvalue slot `i` of the native function currently on top of the
    /// dispatch chain (the one whose body is executing). Returns `Value::Nil`
    /// when no native is running. Public so the C ABI trampoline can fetch
    /// the host C function pointer it stashed there at registration time.
    pub fn running_native_upvalue(&self, i: usize) -> Value {
        match self.running_natives.last() {
            Some(a) => a.nc.upvals.get(i).copied().unwrap_or(Value::Nil),
            None => Value::Nil,
        }
    }

    // ---- native helpers (used by builtins) ----

    /// A native function's own captured upvalue (self lives at func_slot).
    ///
    /// Public so `native_typed` trampolines and embedders authoring
    /// stateful natives via `native_with(...)` can read their upvals.
    pub fn nat_upval(&self, func_slot: u32, i: usize) -> Value {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        nc.upvals[i]
    }

    /// Number of upvalues captured by the native at `func_slot` (variadic
    /// captures such as the `io.lines` format list).
    pub(crate) fn nat_upcount(&self, func_slot: u32) -> usize {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        nc.upvals.len()
    }

    /// Write a native function's own upvalue (stateful iterators).
    pub(crate) fn nat_set_upval(&mut self, func_slot: u32, i: usize, v: Value) {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        // SAFETY: `nc` is the native running at `func_slot`, kept alive by that stack slot; no reference into it is live, and the borrow covers one store
        unsafe { nc.as_mut() }.upvals[i] = v;
        // NativeClosure.upvals is traced as part of its Trace; a long-lived
        // stateful iterator closure (e.g. string.gmatch) sees many writes —
        // barrier_back once-and-done is cheaper than per-child forward.
        self.heap.barrier_back(nc);
    }

    /// Read the i-th positional argument inside a `NativeFn` body
    /// (analogous to `lua_tovalue(L, i + 1)`). `i >= nargs` yields `Nil`,
    /// matching PUC's "missing arg is nil" contract. Public so embedders
    /// can author their own natives.
    pub fn nat_arg(&self, func_slot: u32, nargs: u32, i: u32) -> Value {
        if i < nargs {
            self.stack[(func_slot + 1 + i) as usize]
        } else {
            Value::Nil
        }
    }

    /// Overwrite the i-th argument slot of the running native (the in-place
    /// conversion `lua_tolstring` performs on a number argument).
    pub(crate) fn nat_set_arg(&mut self, func_slot: u32, i: u32, v: Value) {
        self.stack[(func_slot + 1 + i) as usize] = v;
    }

    /// Push the return values of a `NativeFn` and return their count
    /// (analogous to pushing N values then `return N` from a C function).
    /// Public so embedders can author their own natives.
    pub fn nat_return(&mut self, func_slot: u32, vals: &[Value]) -> u32 {
        let need = func_slot as usize + vals.len();
        if self.stack.len() < need {
            self.stack.resize_or_abort(need, Value::Nil);
        }
        for (i, &v) in vals.iter().enumerate() {
            self.stack[func_slot as usize + i] = v;
        }
        vals.len() as u32
    }
}
