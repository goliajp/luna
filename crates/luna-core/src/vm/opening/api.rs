//! The C API calls of the script, each as PUC's makes it: what it
//! allocates, and where it checks for a collection.

use super::run::{Ix, Opener};
use crate::runtime::{Builtin, Gc, NativeClosure, Table, Value};
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Opener {
    /// `lua_createtable`.
    pub(crate) fn createtable(
        &mut self,
        vm: &mut Vm,
        narr: usize,
        nrec: usize,
    ) -> Result<(), LuaError> {
        self.before(vm)?;
        let t = vm.host_new_table(narr, nrec)?;
        self.push(vm, Value::Table(t))?;
        self.after(vm)
    }

    /// `lua_pushstring` / `lua_pushlstring`.
    pub(crate) fn pushstring(&mut self, vm: &mut Vm, b: &[u8]) -> Result<(), LuaError> {
        self.before(vm)?;
        let s = vm.str_value(b)?;
        self.push(vm, s)?;
        self.after(vm)
    }

    /// `lua_pushvalue`.
    pub(crate) fn pushvalue(&mut self, vm: &mut Vm, idx: i64) -> Result<(), LuaError> {
        let v = self.get(vm, idx);
        self.push(vm, v)
    }

    /// `lua_setfield`: the value on top stored under `k` (with
    /// metamethods), popped.
    pub(crate) fn setfield(&mut self, vm: &mut Vm, ix: Ix, k: &[u8]) -> Result<(), LuaError> {
        let t = self.at(vm, ix);
        let key = vm.str_value(k)?;
        let top = self.top(vm);
        let v = vm.stack[top as usize - 1];
        match t {
            // no metatable, so no `__newindex`: the store `luaV_settable`
            // makes then
            Value::Table(tt) if tt.metatable().is_none() => vm.raw_set(tt, key, v)?,
            Value::LightUserdata(p) => {
                if let (Value::Str(s), Value::LightUserdata(c)) = (key, v) {
                    // SAFETY: a light userdata the opening stores into
                    // another is one of its records, as both are here
                    unsafe { vm.heap.env51_set_key(p as *mut _, s, c as *mut _) };
                }
            }
            _ => vm.host_set_index(t, key, v)?,
        }
        self.pop(vm, 1);
        Ok(())
    }

    /// `lua_getfield`: `t[k]` (with metamethods) pushed.
    pub(crate) fn getfield(&mut self, vm: &mut Vm, ix: Ix, k: &[u8]) -> Result<(), LuaError> {
        let t = self.at(vm, ix);
        let key = vm.str_value(k)?;
        let v = match t {
            // no metatable, so no `__index`: the read `luaV_fastget` makes
            Value::Table(tt) if tt.metatable().is_none() => tt.get(key),
            _ => vm.host_index(t, key)?,
        };
        self.push(vm, v)
    }

    /// `lua_setglobal`.
    pub(crate) fn setglobal(&mut self, vm: &mut Vm, k: &[u8]) -> Result<(), LuaError> {
        self.setfield(vm, Ix::Globals, k)
    }

    /// `lua_settable`: the value on top stored under the key below it.
    pub(crate) fn settable(&mut self, vm: &mut Vm, ix: Ix) -> Result<(), LuaError> {
        let t = self.at(vm, ix);
        let top = self.top(vm) as usize;
        let (k, v) = (vm.stack[top - 2], vm.stack[top - 1]);
        vm.host_set_index(t, k, v)?;
        self.pop(vm, 2);
        Ok(())
    }

    /// `lua_rawget`: the key on top replaced by its value in `ix`.
    pub(crate) fn rawget(&mut self, vm: &mut Vm, ix: Ix) {
        let top = self.top(vm) as usize;
        if let Value::Table(t) = self.at(vm, ix) {
            vm.stack[top - 1] = t.get(vm.stack[top - 1]);
        }
    }

    /// `lua_rawseti`: the value on top stored at `n`, popped.
    pub(crate) fn rawseti(&mut self, vm: &mut Vm, ix: Ix, n: i64) -> Result<(), LuaError> {
        let top = self.top(vm) as usize;
        let v = vm.stack[top - 1];
        match self.at(vm, ix) {
            Value::Table(t) => vm.host_raw_set(t, Value::Int(n), v)?,
            e => self.phantom_seti(vm, e, n, v),
        }
        self.pop(vm, 1);
        Ok(())
    }

    /// `lua_rawgeti`.
    pub(crate) fn rawgeti(&mut self, vm: &mut Vm, ix: Ix, n: i64) -> Result<(), LuaError> {
        let v = match self.at(vm, ix) {
            Value::Table(t) => t.get(Value::Int(n)),
            _ => Value::Nil,
        };
        self.push(vm, v)
    }

    /// `lua_rawsetp`: the value on top stored under the address PUC keys
    /// its table of C libraries by (5.3), popped.
    pub(crate) fn rawsetp(&mut self, vm: &mut Vm, ix: Ix) -> Result<(), LuaError> {
        let top = self.top(vm) as usize;
        let v = vm.stack[top - 1];
        if let Value::Table(t) = self.at(vm, ix) {
            let k = crate::vm::exec::light_key(&crate::vm::exec::CLIBS_KEY);
            vm.host_raw_set(t, k, v)?;
        }
        self.pop(vm, 1);
        Ok(())
    }

    /// `lua_setmetatable`: the table on top made the metatable of `ix`,
    /// popped.
    pub(crate) fn setmetatable(&mut self, vm: &mut Vm, ix: Ix) -> Result<(), LuaError> {
        let obj = self.at(vm, ix);
        let top = self.top(vm) as usize;
        let mt = match vm.stack[top - 1] {
            Value::Table(t) => Some(t),
            _ => None,
        };
        vm.host_set_metatable(obj, mt)?;
        self.pop(vm, 1);
        Ok(())
    }

    /// `lua_remove`.
    pub(crate) fn remove(&mut self, vm: &mut Vm, idx: i64) {
        let Ix::Slot(s) = self.ix(vm, idx) else {
            unreachable!("a stack index")
        };
        let top = self.top(vm) as usize;
        vm.stack.copy_within(s as usize + 1..top, s as usize);
        self.pop(vm, 1);
    }

    /// `lua_replace`: the value on top moved to `ix`.
    pub(crate) fn replace(&mut self, vm: &mut Vm, ix: Ix) -> Result<(), LuaError> {
        let top = self.top(vm) as usize;
        let v = vm.stack[top - 1];
        match ix {
            Ix::Slot(s) => vm.stack[s as usize] = v,
            Ix::Env => self.set_env(vm, v),
            _ => unreachable!("5.1 replaces only the environment"),
        }
        self.pop(vm, 1);
        Ok(())
    }

    /// `lua_call`: the function below `nargs` arguments called, its
    /// results (`nres`, or all when negative) left in its place.
    pub(crate) fn call(&mut self, vm: &mut Vm, nargs: u32, nres: i32) -> Result<(), LuaError> {
        let top = self.top(vm);
        let at = top - nargs - 1;
        let f = vm.stack[at as usize];
        let args = vm.stack[at as usize + 1..top as usize].to_vec();
        let mut vals = vm.call_value_at(f, &args, at)?;
        if nres >= 0 {
            vals.resize(nres as usize, Value::Nil);
        }
        self.set_top(vm, at);
        for v in vals {
            self.push(vm, v)?;
        }
        Ok(())
    }

    /// `lua_pushcclosure` of the function the script numbers `f`, with
    /// `nup` upvalues on top: from 5.2 on one without is a light function,
    /// which allocates nothing.
    pub(crate) fn pushcclosure(&mut self, vm: &mut Vm, f: u32, nup: u32) -> Result<(), LuaError> {
        let counted = nup > 0 || self.v == LuaVersion::Lua51;
        if counted {
            self.before(vm)?;
        }
        let fv = self.make_fn(vm, f, nup)?;
        if nup > 0 {
            // the closure takes the first upvalue's slot
            let t = self.top(vm);
            vm.stack[(t - nup) as usize] = fv;
            self.pop(vm, nup - 1);
        } else {
            self.push(vm, fv)?;
        }
        if counted {
            self.after(vm)?;
        }
        Ok(())
    }

    /// The function PUC's script makes as `id` with `nup` upvalues: luna's
    /// native `f` over `upvals`. From 5.2 on, one PUC makes without
    /// upvalues is its light function: one value for every push of it,
    /// counted as nothing.
    #[inline]
    pub(crate) fn func(
        &mut self,
        vm: &mut Vm,
        id: u32,
        f: crate::runtime::value::NativeFn,
        b: Builtin,
        upvals: &[Value],
        nup: u32,
    ) -> Result<Value, LuaError> {
        if self.v != LuaVersion::Lua51 && nup == 0 {
            if upvals.is_empty()
                && let Some(n) = vm.heap.light(id as usize)
            {
                return Ok(Value::Native(n));
            }
            let fixing = vm.heap.fix_natives();
            let n = vm.builtin(f, upvals, b);
            vm.heap.stop_fixing_natives(fixing);
            let Value::Native(n) = n? else {
                unreachable!("a native")
            };
            vm.heap.set_native_puc_nup(n, 0);
            if upvals.is_empty() {
                vm.heap.set_light(id as usize, super::FN_COUNT, n);
            }
            return Ok(Value::Native(n));
        }
        let n: Gc<NativeClosure> =
            vm.retry_oom(|vm| vm.heap.try_new_native_puc(f, upvals, b, nup as usize))?;
        if self.v == LuaVersion::Lua51 {
            let env = self.env_bits(vm);
            vm.heap.set_env51(n.hdr(), env);
        }
        Ok(Value::Native(n))
    }

    /// The global `name`, read raw without making the name.
    pub(crate) fn global(&self, vm: &Vm, name: &[u8]) -> Value {
        let g: Gc<Table> = vm.globals();
        vm.heap
            .find_interned(name)
            .map_or(Value::Nil, |k| g.get(Value::Str(k)))
    }

    /// Whether a `luaL_register` without a library name runs (5.1's file
    /// methods).
    pub(crate) fn registering_methods(&self) -> bool {
        self.methods
    }
}
