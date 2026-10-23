//! What a host does to Lua values as Lua code does: index and assign with
//! metamethods, raise an error with the running code's position.

use super::*;

impl Vm {
    /// `t[key]` with metamethods, as the Lua expression does (PUC
    /// `lua_gettable`): `__index` is followed or called when `t` is not a
    /// table or has no such key.
    ///
    /// # Errors
    ///
    /// Indexing a value that cannot be indexed, or an error an `__index`
    /// function raises.
    ///
    /// # Example
    ///
    /// ```
    /// use luna_core::runtime::Value;
    /// use luna_core::version::LuaVersion;
    /// use luna_core::vm::Vm;
    ///
    /// let mut vm = Vm::new(LuaVersion::Lua54);
    /// let t = vm.eval("return setmetatable({}, {__index = function(_, k) return k * 2 end})").unwrap()[0];
    /// let v = vm.index_with_mm(t, Value::Int(21)).unwrap();
    /// assert!(matches!(v, Value::Int(42)));
    /// ```
    pub fn index_with_mm(&mut self, t: Value, key: Value) -> Result<Value, LuaError> {
        self.index_value(t, key)
    }

    /// `t[key] = v` with metamethods, as the Lua assignment does (PUC
    /// `lua_settable`): `__newindex` is followed or called when `t` is not
    /// a table or has no such key.
    ///
    /// # Errors
    ///
    /// Indexing a value that cannot be indexed, a key a table cannot hold
    /// (nil, NaN), or an error a `__newindex` function raises.
    ///
    /// # Example
    ///
    /// ```
    /// use luna_core::runtime::Value;
    /// use luna_core::version::LuaVersion;
    /// use luna_core::vm::Vm;
    ///
    /// let mut vm = Vm::new(LuaVersion::Lua54);
    /// let t = vm
    ///     .eval("seen = {} return setmetatable({}, {__newindex = function(_, k, v) seen[k] = v end})")
    ///     .unwrap()[0];
    /// vm.set_index_with_mm(t, Value::Int(1), Value::Bool(true)).unwrap();
    /// let seen = vm.eval("return seen[1]").unwrap();
    /// assert!(matches!(seen[..], [Value::Bool(true)]));
    /// ```
    pub fn set_index_with_mm(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        self.newindex_value(t, key, v)
    }

    /// A runtime error with message `msg`, as a library function raises it
    /// (PUC `luaL_error`): the position of the Lua code running, if any,
    /// goes before the message (`chunk:line: msg`). A native function
    /// returns it as its error.
    ///
    /// # Example
    ///
    /// ```
    /// use luna_core::version::LuaVersion;
    /// use luna_core::vm::{LuaError, Vm};
    ///
    /// let mut vm = Vm::new(LuaVersion::Lua54);
    /// fn fail(vm: &mut Vm, _: u32, _: u32) -> Result<u32, LuaError> {
    ///     Err(vm.runtime_error("not today"))
    /// }
    /// let f = vm.native(fail);
    /// vm.set_global("fail", f).unwrap();
    /// let e = vm.eval("local ok, m = pcall(fail) return m == 'not today'").unwrap();
    /// assert!(matches!(e[..], [luna_core::runtime::Value::Bool(true)]));
    /// ```
    pub fn runtime_error(&mut self, msg: &str) -> LuaError {
        self.rt_err(msg)
    }
}
