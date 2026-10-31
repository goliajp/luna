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
        match self.nat_ret(func_slot, vals) {
            Ok(n) => n,
            Err(_) => crate::runtime::mem::oom_abort(std::alloc::Layout::for_value(vals)),
        }
    }

    /// [`Vm::nat_return`], raising a memory error when the stack cannot
    /// grow to hold the results.
    #[inline(always)]
    pub fn nat_ret(&mut self, func_slot: u32, vals: &[Value]) -> Result<u32, LuaError> {
        let need = func_slot as usize + vals.len();
        if self.stack.len() < need {
            self.grow_stack_holding(need, vals)?;
        }
        // in pieces: the caller has just made the values (see `nat_ret1`)
        for (i, v) in vals.iter().enumerate() {
            Value::set_split(&mut self.stack[func_slot as usize + i], v);
        }
        Ok(vals.len() as u32)
    }

    /// [`Vm::nat_ret`] of the one value `v`. The growing path roots `v`,
    /// so `v` sits in memory, written as a tag byte and a payload word; it
    /// goes to the result slot in the same two pieces (`Value::copy_raw`),
    /// since a whole-value copy would wait for those stores on every call.
    #[inline(always)]
    pub(crate) fn nat_ret1(&mut self, func_slot: u32, v: Value) -> Result<u32, LuaError> {
        match self.stack.get_mut(func_slot as usize) {
            Some(slot) => {
                Value::set_split(slot, &v);
                Ok(1)
            }
            None => self.nat_ret1_grow(func_slot, v),
        }
    }

    #[cold]
    #[inline(never)]
    fn nat_ret1_grow(&mut self, func_slot: u32, v: Value) -> Result<u32, LuaError> {
        self.nat_ret1(func_slot, v)
    }

    /// The stack grown to `need` slots with `vals`, which the caller holds
    /// to put there, rooted meanwhile: growing may collect in an emergency.
    #[inline]
    pub(crate) fn grow_stack_holding(
        &mut self,
        need: usize,
        vals: &[Value],
    ) -> Result<(), LuaError> {
        if self.stack.len() >= need {
            return Ok(());
        }
        self.grow_stack_holding_slow(need, vals)
    }

    #[cold]
    #[inline(never)]
    fn grow_stack_holding_slow(&mut self, need: usize, vals: &[Value]) -> Result<(), LuaError> {
        self.rooted(vals, |vm| vm.grow_stack(need))
    }
}
