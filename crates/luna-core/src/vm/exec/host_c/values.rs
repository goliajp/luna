//! Values for the C API: raw table access, metatables, arithmetic,
//! comparison, concatenation and length with metamethods, and 5.1
//! environments.
//!
//! An operation the C API makes runs its metamethods as a C function's call
//! does in PUC (`luaD_callnoyield`): a yield inside one is refused.

use super::*;
use crate::runtime::{LuaClosure, NativeClosure, UpvalState};

/// `lua_arith`'s operations, in 5.3+ `LUA_OP*` order.
const ARITH_OPS: [ArithOp; 12] = [
    ArithOp::Add,
    ArithOp::Sub,
    ArithOp::Mul,
    ArithOp::Mod,
    ArithOp::Pow,
    ArithOp::Div,
    ArithOp::IDiv,
    ArithOp::BAnd,
    ArithOp::BOr,
    ArithOp::BXor,
    ArithOp::Shl,
    ArithOp::Shr,
];

#[doc(hidden)]
/// 5.3+ `LUA_OPUNM`.
pub const HOST_OP_UNM: u8 = 12;
#[doc(hidden)]
/// 5.3+ `LUA_OPBNOT`.
pub const HOST_OP_BNOT: u8 = 13;

#[doc(hidden)]
impl Vm {
    fn host_noyield<R>(&mut self, f: impl FnOnce(&mut Vm) -> R) -> R {
        self.nny += 1;
        let r = f(self);
        self.nny -= 1;
        r
    }

    /// `t[k]` with metamethods (PUC `lua_gettable`).
    pub fn host_index(&mut self, t: Value, k: Value) -> Result<Value, LuaError> {
        self.host_noyield(|vm| vm.index_value(t, k))
    }

    /// `t[k] = v` with metamethods (PUC `lua_settable`).
    pub fn host_set_index(&mut self, t: Value, k: Value, v: Value) -> Result<(), LuaError> {
        self.host_noyield(|vm| vm.newindex_value(t, k, v))
    }

    /// `t[k] = v` without metamethods (PUC `lua_rawset`): a nil or NaN key
    /// and a read-only table raise.
    pub fn host_raw_set(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), LuaError> {
        self.raw_set(t, k, v)
    }

    /// A new table with room for `narr` array items and `nrec` others
    /// (PUC `lua_createtable`).
    pub fn host_new_table(&mut self, narr: usize, nrec: usize) -> Gc<Table> {
        let t = self.heap.new_table();
        // SAFETY: `t` was allocated above and is held only by this local;
        // the borrows cover two resizes, which do not collect
        unsafe {
            if narr > 0 {
                t.as_mut().ensure_array(&mut self.heap, narr);
            }
            if nrec > 0 {
                t.as_mut().ensure_hash(&mut self.heap, nrec);
            }
        }
        t
    }

    /// The entry of `t` after key `k` (PUC `lua_next`); an absent key
    /// raises "invalid key to 'next'".
    pub fn host_next(
        &mut self,
        t: Gc<Table>,
        k: Value,
    ) -> Result<Option<(Value, Value)>, LuaError> {
        t.next(k).map_err(|e| self.table_error(e))
    }

    /// The metatable of `v` (PUC `lua_getmetatable`): its own, or its
    /// type's.
    pub fn host_metatable(&self, v: Value) -> Option<Gc<Table>> {
        self.metatable_of(v)
    }

    /// Set the metatable of `v` (PUC `lua_setmetatable`), or its type's for
    /// a value that has no metatable of its own. From 5.2 on an object
    /// whose new metatable has `__gc` is marked for finalization now; 5.1
    /// marks every userdata with a metatable, and looks for `__gc` when it
    /// collects it. A read-only table refuses, as Redis's
    /// `lua_setmetatable` does.
    pub fn host_set_metatable(&mut self, v: Value, mt: Option<Gc<Table>>) -> Result<(), LuaError> {
        match v {
            Value::Table(t) => {
                self.refuse_readonly(t)?;
                // SAFETY: `t` is held by the caller's stack slot; the borrow
                // covers one store and `mt` is a separate handle
                unsafe { t.as_mut() }.set_metatable(mt);
                self.heap.barrier_back(t);
                if mt.is_some() {
                    self.check_finalizer(t);
                }
            }
            Value::Userdata(u) => {
                // SAFETY: as for a table
                unsafe { u.as_mut() }.set_metatable(mt);
                self.heap.barrier_back(u);
                if mt.is_some() {
                    if self.version == LuaVersion::Lua51 {
                        self.heap.register_finalizable_userdata(u);
                    } else {
                        self.check_finalizer_userdata(u);
                    }
                }
            }
            _ => self.set_type_metatable(v, mt),
        }
        Ok(())
    }

    /// PUC `lua_arith` with the 5.3+ operation `op` (0 to 13): `r` is the
    /// second operand, and the operand itself again for the unary ones.
    pub fn host_arith(&mut self, op: u8, l: Value, r: Value) -> Result<Value, LuaError> {
        match op {
            HOST_OP_UNM => self.host_unm(l),
            HOST_OP_BNOT => self.host_bnot(l),
            _ => {
                let op = ARITH_OPS[op as usize];
                if let Some(v) = self.arith_fast(op, l, r)? {
                    return Ok(v);
                }
                let mm = self.arith_mm_func(op, l, r)?;
                self.host_noyield(|vm| vm.call_mm1(mm, &[l, r]))
            }
        }
    }

    fn host_unm(&mut self, v: Value) -> Result<Value, LuaError> {
        match self.unary_operand(v) {
            Some(Num::Int(i)) if self.version <= LuaVersion::Lua52 => Ok(super::num_double::neg(i)),
            Some(Num::Int(i)) => Ok(Value::Int(i.wrapping_neg())),
            Some(Num::Float(f)) => Ok(Value::Float(-f)),
            None => {
                let mm = self.get_mm(v, Mm::Unm);
                if mm.is_nil() {
                    return Err(self.type_err("perform arithmetic on", v));
                }
                self.host_noyield(|vm| vm.call_mm1(mm, &[v, v]))
            }
        }
    }

    fn host_bnot(&mut self, v: Value) -> Result<Value, LuaError> {
        match self.arith_operand()(v) {
            Some(n) => match int_of(n) {
                Some(i) => Ok(Value::Int(!i)),
                None => Err(self.no_int_rep_err()),
            },
            None => {
                let mm = self.get_mm(v, Mm::BNot);
                if mm.is_nil() {
                    return Err(self.type_err("perform bitwise operation on", v));
                }
                self.host_noyield(|vm| vm.call_mm1(mm, &[v, v]))
            }
        }
    }

    /// `l == r` with `__eq` (PUC `lua_compare` with `LUA_OPEQ`, 5.1
    /// `lua_equal`).
    pub fn host_equal(&mut self, l: Value, r: Value) -> Result<bool, LuaError> {
        self.host_noyield(|vm| vm.equal(l, r))
    }

    /// `l < r`, or `l <= r` with `or_eq`, with metamethods (PUC
    /// `lua_compare`, 5.1 `lua_lessthan`).
    pub fn host_less(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError> {
        self.host_noyield(|vm| vm.less_than(l, r, or_eq))
    }

    /// `#v` with `__len` (PUC `lua_len`).
    pub fn host_len(&mut self, v: Value) -> Result<Value, LuaError> {
        self.host_noyield(|vm| vm.len_value(v))
    }

    /// The concatenation of `vals`, at least two, right to left with
    /// `__concat` (PUC `luaV_concat`).
    pub fn host_concat(&mut self, vals: &[Value]) -> Result<Value, LuaError> {
        let mut acc = *vals.last().expect("lua_concat of at least two values");
        for &x in vals[..vals.len() - 1].iter().rev() {
            acc = match self.concat_pair(x, acc)? {
                Some(s) => s,
                None => {
                    let mut mm = self.get_mm(x, Mm::Concat);
                    if mm.is_nil() {
                        mm = self.get_mm(acc, Mm::Concat);
                    }
                    if mm.is_nil() {
                        let bad = if concat_piece(x, self.float_fmt()).is_none() {
                            x
                        } else {
                            acc
                        };
                        return Err(self.type_err("concatenate", bad));
                    }
                    let y = acc;
                    self.host_noyield(|vm| vm.call_mm1(mm, &[x, y]))?
                }
            };
        }
        Ok(acc)
    }

    /// A step of the collector if one is due (PUC `luaC_checkGC`): every
    /// value the C API holds is on a C stack, which the collector marks.
    pub fn host_check_gc(&mut self) {
        let top = self.stack.len() as u32;
        self.maybe_collect_garbage(top);
    }

    /// The 5.1 environment of the Lua function `cl`: its `_ENV` cell, or the
    /// globals when it has none.
    pub fn host_closure_env(&self, cl: Gc<LuaClosure>) -> Value {
        match cl.proto.upvals.iter().position(|d| &*d.name == "_ENV") {
            Some(i) => match cl.upvals()[i].state() {
                UpvalState::Closed(v) => v,
                UpvalState::Open { slot, thread } => self.read_slot(slot, thread),
            },
            None => Value::Table(self.globals),
        }
    }

    /// Give the Lua function `cl` the 5.1 environment `env` (PUC
    /// `lua_setfenv`); `false` when it has no `_ENV` cell, as luna keeps
    /// a 5.1 environment only in that cell.
    pub fn host_set_closure_env(&mut self, cl: Gc<LuaClosure>, env: Gc<Table>) -> bool {
        match cl.proto.upvals.iter().position(|d| &*d.name == "_ENV") {
            Some(i) => {
                self.set_closure_env(cl, i, env);
                true
            }
            None => false,
        }
    }

    /// Store `v` in upvalue `i` of the native `nc`.
    pub fn host_set_native_upvalue(&mut self, nc: Gc<NativeClosure>, i: usize, v: Value) {
        // SAFETY: `nc` is held by the caller; no other reference into it is
        // live, and the borrow covers one store
        unsafe { nc.as_mut() }.upvals[i] = v;
        self.heap.barrier_back(nc);
    }
}
