//! The C API's view of a Vm: values, tables, userdata blocks and the checks that allocate.

use crate::runtime::*;
use crate::vm::LuaError;
use crate::vm::exec::host_c::*;

/// What the C API (`luna-jit`'s `capi`) asks of a Vm: values, tables, userdata blocks and the checks that allocate.
pub trait VmCApiValues {
    /// `t[k]` with metamethods (PUC `lua_gettable`).
    fn host_index(&mut self, t: Value, k: Value) -> Result<Value, LuaError>;
    /// `t[k] = v` with metamethods (PUC `lua_settable`).
    fn host_set_index(&mut self, t: Value, k: Value, v: Value) -> Result<(), LuaError>;
    /// `t[k] = v` without metamethods (PUC `lua_rawset`): a nil or NaN key
    /// and a read-only table raise.
    fn host_raw_set(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), LuaError>;
    /// A new table with room for `narr` array items and `nrec` others
    /// (PUC `lua_createtable`).
    fn host_new_table(&mut self, narr: usize, nrec: usize) -> Result<Gc<Table>, LuaError>;
    /// The table 5.5's `lua_load` anchors the parser's work in, on the
    /// stack under what the reader pushes: luna's own, uncounted, as the
    /// load counts PUC's in its place.
    fn host_load_anchor(&mut self) -> Result<Gc<Table>, LuaError>;
    /// The entry of `t` after key `k` (PUC `lua_next`); an absent key
    /// raises "invalid key to 'next'".
    fn host_next(&mut self, t: Gc<Table>, k: Value) -> Result<Option<(Value, Value)>, LuaError>;
    /// The metatable of `v` (PUC `lua_getmetatable`): its own, or its
    /// type's.
    fn host_metatable(&self, v: Value) -> Option<Gc<Table>>;
    /// Set the metatable of `v` (PUC `lua_setmetatable`), or its type's for
    /// a value that has no metatable of its own. From 5.2 on an object
    /// whose new metatable has `__gc` is marked for finalization now; 5.1
    /// marks every userdata with a metatable, and looks for `__gc` when it
    /// collects it. A read-only table refuses, as Redis's
    /// `lua_setmetatable` does.
    fn host_set_metatable(&mut self, v: Value, mt: Option<Gc<Table>>) -> Result<(), LuaError>;
    /// PUC `lua_arith` with the 5.3+ operation `op` (0 to 13): `r` is the
    /// second operand, and the operand itself again for the unary ones.
    fn host_arith(&mut self, op: u8, l: Value, r: Value) -> Result<Value, LuaError>;
    /// `l == r` with `__eq` (PUC `lua_compare` with `LUA_OPEQ`, 5.1
    /// `lua_equal`).
    fn host_equal(&mut self, l: Value, r: Value) -> Result<bool, LuaError>;
    /// `l < r`, or `l <= r` with `or_eq`, with metamethods (PUC
    /// `lua_compare`, 5.1 `lua_lessthan`).
    fn host_less(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError>;
    /// `#v` with `__len` (PUC `lua_len`).
    fn host_len(&mut self, v: Value) -> Result<Value, LuaError>;
    /// The concatenation of `vals`, at least two, right to left with
    /// `__concat` (PUC `luaV_concat`).
    fn host_concat(&mut self, vals: &[Value]) -> Result<Value, LuaError>;
    /// A step of the collector if one is due (PUC `luaC_checkGC`): every
    /// value the C API holds is on a C stack, which the collector marks.
    fn host_check_gc(&mut self) -> Result<(), LuaError>;
    /// The check of an API call that allocates, made before it in 5.1 and
    /// 5.2 (`before`) and after it pushed from 5.3 on (`!before`).
    fn host_api_check(&mut self, before: bool) -> Result<(), LuaError>;
    /// The 5.1 environment of the Lua function `cl`: its `_ENV` cell, or the
    /// globals when it has none.
    fn host_closure_env(&self, cl: Gc<LuaClosure>) -> Value;
    /// Give the Lua function `cl` the 5.1 environment `env` (PUC
    /// `lua_setfenv`); `false` when it has no `_ENV` cell, as luna keeps
    /// a 5.1 environment only in that cell, or when there is no memory for
    /// the new cell.
    fn host_set_closure_env(&mut self, cl: Gc<LuaClosure>, env: Gc<Table>) -> bool;
    /// Store `v` in upvalue `i` of the native `nc`.
    fn host_set_native_upvalue(&mut self, nc: Gc<NativeClosure>, i: usize, v: Value);
    /// `bytes` as a string value.
    fn host_str(&mut self, bytes: &[u8]) -> Result<Value, LuaError>;
    /// The memory error (PUC `LUA_ERRMEM`, "not enough memory").
    fn host_mem_error(&mut self) -> LuaError;
    /// An emergency collection after an allocation failed, where the
    /// dialect runs one; `false` when none ran.
    fn host_emergency_collect(&mut self) -> bool;
    /// Make room for `n` more values on `co`'s C stack, collecting in an
    /// emergency when memory runs out; `false` when there is none.
    fn host_reserve_cstack(&mut self, co: Gc<Coro>, n: usize) -> bool;
    /// A new full userdata of `size` bytes, zeroed, with `nuv` nil user
    /// values and no metatable; a memory error when it cannot be made, after
    /// an emergency collection.
    fn host_new_block(
        &mut self,
        size: usize,
        nuv: usize,
    ) -> Result<Gc<crate::runtime::Userdata>, LuaError>;
    /// The C API block of userdata `u`, if the C API made it.
    fn host_block(&self, u: Gc<crate::runtime::Userdata>) -> Option<&HostBlock>;
    /// The address C sees for userdata `u` (`lua_touserdata`): its block,
    /// or the object itself for a userdata the C API did not make.
    fn host_userdata_ptr(&self, u: Gc<crate::runtime::Userdata>) -> *mut u8;
    /// User value `n` (from 1) of `u`; `None` when it has no such value.
    /// A userdata the C API did not make has the one 5.2/5.3 slot, and none
    /// from 5.4 on (PUC's library userdata are made with no user values).
    fn host_uservalue(&self, u: Gc<crate::runtime::Userdata>, n: usize) -> Option<Value>;
    /// Set user value `n` (from 1) of `u`; `false` when it has no such
    /// value.
    fn host_set_uservalue(&mut self, u: Gc<crate::runtime::Userdata>, n: usize, v: Value) -> bool;
}

impl VmCApiValues for crate::vm::Vm {
    #[inline]
    fn host_index(&mut self, t: Value, k: Value) -> Result<Value, LuaError> {
        crate::vm::Vm::host_index(self, t, k)
    }
    #[inline]
    fn host_set_index(&mut self, t: Value, k: Value, v: Value) -> Result<(), LuaError> {
        crate::vm::Vm::host_set_index(self, t, k, v)
    }
    #[inline]
    fn host_raw_set(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), LuaError> {
        crate::vm::Vm::host_raw_set(self, t, k, v)
    }
    #[inline]
    fn host_new_table(&mut self, narr: usize, nrec: usize) -> Result<Gc<Table>, LuaError> {
        crate::vm::Vm::host_new_table(self, narr, nrec)
    }
    #[inline]
    fn host_load_anchor(&mut self) -> Result<Gc<Table>, LuaError> {
        crate::vm::Vm::host_load_anchor(self)
    }
    #[inline]
    fn host_next(&mut self, t: Gc<Table>, k: Value) -> Result<Option<(Value, Value)>, LuaError> {
        crate::vm::Vm::host_next(self, t, k)
    }
    #[inline]
    fn host_metatable(&self, v: Value) -> Option<Gc<Table>> {
        crate::vm::Vm::host_metatable(self, v)
    }
    #[inline]
    fn host_set_metatable(&mut self, v: Value, mt: Option<Gc<Table>>) -> Result<(), LuaError> {
        crate::vm::Vm::host_set_metatable(self, v, mt)
    }
    #[inline]
    fn host_arith(&mut self, op: u8, l: Value, r: Value) -> Result<Value, LuaError> {
        crate::vm::Vm::host_arith(self, op, l, r)
    }
    #[inline]
    fn host_equal(&mut self, l: Value, r: Value) -> Result<bool, LuaError> {
        crate::vm::Vm::host_equal(self, l, r)
    }
    #[inline]
    fn host_less(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError> {
        crate::vm::Vm::host_less(self, l, r, or_eq)
    }
    #[inline]
    fn host_len(&mut self, v: Value) -> Result<Value, LuaError> {
        crate::vm::Vm::host_len(self, v)
    }
    #[inline]
    fn host_concat(&mut self, vals: &[Value]) -> Result<Value, LuaError> {
        crate::vm::Vm::host_concat(self, vals)
    }
    #[inline]
    fn host_check_gc(&mut self) -> Result<(), LuaError> {
        crate::vm::Vm::host_check_gc(self)
    }
    #[inline]
    fn host_api_check(&mut self, before: bool) -> Result<(), LuaError> {
        crate::vm::Vm::host_api_check(self, before)
    }
    #[inline]
    fn host_closure_env(&self, cl: Gc<LuaClosure>) -> Value {
        crate::vm::Vm::host_closure_env(self, cl)
    }
    #[inline]
    fn host_set_closure_env(&mut self, cl: Gc<LuaClosure>, env: Gc<Table>) -> bool {
        crate::vm::Vm::host_set_closure_env(self, cl, env)
    }
    #[inline]
    fn host_set_native_upvalue(&mut self, nc: Gc<NativeClosure>, i: usize, v: Value) {
        crate::vm::Vm::host_set_native_upvalue(self, nc, i, v)
    }
    #[inline]
    fn host_str(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        crate::vm::Vm::host_str(self, bytes)
    }
    #[inline]
    fn host_mem_error(&mut self) -> LuaError {
        crate::vm::Vm::host_mem_error(self)
    }
    #[inline]
    fn host_emergency_collect(&mut self) -> bool {
        crate::vm::Vm::host_emergency_collect(self)
    }
    #[inline]
    fn host_reserve_cstack(&mut self, co: Gc<Coro>, n: usize) -> bool {
        crate::vm::Vm::host_reserve_cstack(self, co, n)
    }
    #[inline]
    fn host_new_block(
        &mut self,
        size: usize,
        nuv: usize,
    ) -> Result<Gc<crate::runtime::Userdata>, LuaError> {
        crate::vm::Vm::host_new_block(self, size, nuv)
    }
    #[inline]
    fn host_block(&self, u: Gc<crate::runtime::Userdata>) -> Option<&HostBlock> {
        crate::vm::Vm::host_block(self, u)
    }
    #[inline]
    fn host_userdata_ptr(&self, u: Gc<crate::runtime::Userdata>) -> *mut u8 {
        crate::vm::Vm::host_userdata_ptr(self, u)
    }
    #[inline]
    fn host_uservalue(&self, u: Gc<crate::runtime::Userdata>, n: usize) -> Option<Value> {
        crate::vm::Vm::host_uservalue(self, u, n)
    }
    #[inline]
    fn host_set_uservalue(&mut self, u: Gc<crate::runtime::Userdata>, n: usize, v: Value) -> bool {
        crate::vm::Vm::host_set_uservalue(self, u, n, v)
    }
}
