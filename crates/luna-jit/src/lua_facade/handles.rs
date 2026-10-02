//! Pinned handle types and tuple-to-argument conversion.

use luna_core::runtime::Value;
use luna_core::vm::{FromLuaValue, HostRootTicket, IntoValue, LuaError, Vm};

use super::{Lua, PinnedHandle};

/// Handle to a Lua-callable value (`Value::Closure` or
/// `Value::Native`) pinned in the host root pool. `Copy`-able —
/// clones share the same [`HostRootTicket`].
///
/// Becomes stale (panics on call) after [`Lua::unpin`] /
/// [`Lua::unpin_all`].
#[derive(Copy, Clone, Debug)]
pub struct LuaFunction {
    pub(super) ticket: HostRootTicket,
}

impl LuaFunction {
    /// The underlying [`HostRootTicket`]. Facade-author use only.
    pub fn ticket(self) -> HostRootTicket {
        self.ticket
    }

    /// Call this function with the given typed args; decode the
    /// (first) return as `R`. Use [`LuaFunction::call_multi`] for
    /// the full result vector.
    pub fn call<A, R>(self, lua: &mut Lua, args: A) -> Result<R, LuaError>
    where
        A: IntoLuaArgs,
        R: FromLuaValue,
    {
        let f = lua
            .0
            .read_host(self.ticket)
            .expect("LuaFunction used after unpin / unpin_all");
        let args = args.into_lua_args(&mut lua.0);
        let mut r = lua.0.call_value(f, &args)?;
        if r.is_empty() {
            R::from_lua_value(Value::Nil)
        } else {
            R::from_lua_value(r.remove(0))
        }
    }

    /// Call this function; return all results.
    pub fn call_multi<A>(self, lua: &mut Lua, args: A) -> Result<Vec<Value>, LuaError>
    where
        A: IntoLuaArgs,
    {
        let f = lua
            .0
            .read_host(self.ticket)
            .expect("LuaFunction used after unpin / unpin_all");
        let args = args.into_lua_args(&mut lua.0);
        lua.0.call_value(f, &args)
    }
}

impl IntoValue for LuaFunction {
    fn into_value(self, vm: &mut Vm) -> Value {
        vm.read_host(self.ticket)
            .expect("LuaFunction used after unpin / unpin_all")
    }
}

impl PinnedHandle for LuaFunction {
    fn ticket(&self) -> HostRootTicket {
        self.ticket
    }
}

/// Handle to a `Value::Table` pinned in the host root pool.
#[derive(Copy, Clone, Debug)]
pub struct LuaTable {
    pub(super) ticket: HostRootTicket,
}

impl LuaTable {
    /// The underlying [`HostRootTicket`]. Facade-author use only.
    pub fn ticket(self) -> HostRootTicket {
        self.ticket
    }

    /// Set `t[k] = v`. Both `k` and `v` may be any [`IntoValue`].
    pub fn set<K: IntoValue, V: IntoValue>(
        self,
        lua: &mut Lua,
        k: K,
        v: V,
    ) -> Result<(), LuaError> {
        let t = match lua
            .0
            .read_host(self.ticket)
            .expect("LuaTable used after unpin / unpin_all")
        {
            Value::Table(t) => t,
            _ => return Err(LuaError(Value::Nil)),
        };
        let k = k.into_value(&mut lua.0);
        let v = v.into_value(&mut lua.0);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is
        // single-threaded (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut lua.0.heap, k, v)?;
        lua.0
            .heap
            .barrier_back(t.as_ptr() as *mut luna_core::runtime::heap::GcHeader);
        Ok(())
    }

    /// Read `t[k]`; decode as `V`. Returns `Err` if the key is
    /// missing OR the value's type doesn't match `V`. Use
    /// `t.raw_get(k)` (returning `Value`) for runtime branching.
    pub fn get<K: IntoValue, V: FromLuaValue>(self, lua: &mut Lua, k: K) -> Result<V, LuaError> {
        let v = self.raw_get(lua, k)?;
        V::from_lua_value(v)
    }

    /// Read `t[k]` as a raw [`Value`] (no type coercion).
    pub fn raw_get<K: IntoValue>(self, lua: &mut Lua, k: K) -> Result<Value, LuaError> {
        let t = match lua
            .0
            .read_host(self.ticket)
            .expect("LuaTable used after unpin / unpin_all")
        {
            Value::Table(t) => t,
            _ => return Err(LuaError(Value::Nil)),
        };
        let k = k.into_value(&mut lua.0);
        // SAFETY: see set() — same single-threaded GC contract.
        Ok(unsafe { t.as_mut() }.get(k))
    }
}

impl IntoValue for LuaTable {
    fn into_value(self, vm: &mut Vm) -> Value {
        vm.read_host(self.ticket)
            .expect("LuaTable used after unpin / unpin_all")
    }
}

impl PinnedHandle for LuaTable {
    fn ticket(&self) -> HostRootTicket {
        self.ticket
    }
}

/// Generic pinned root. Use for arbitrary `Value`s the embedder
/// wants to keep alive without wrapping in `LuaFunction` / `LuaTable`.
#[derive(Copy, Clone, Debug)]
pub struct LuaRoot {
    pub(super) ticket: HostRootTicket,
}

impl LuaRoot {
    /// The underlying [`HostRootTicket`]. Facade-author use only.
    pub fn ticket(self) -> HostRootTicket {
        self.ticket
    }

    /// Read the pinned value. Panics if the handle was released.
    pub fn get(self, lua: &Lua) -> Value {
        lua.0
            .read_host(self.ticket)
            .expect("LuaRoot used after unpin / unpin_all")
    }
}

impl IntoValue for LuaRoot {
    fn into_value(self, vm: &mut Vm) -> Value {
        vm.read_host(self.ticket)
            .expect("LuaRoot used after unpin / unpin_all")
    }
}

impl PinnedHandle for LuaRoot {
    fn ticket(&self) -> HostRootTicket {
        self.ticket
    }
}

/// Convert a tuple of typed values into the `&[Value]` shape
/// [`Vm::call_value`] expects. Implemented for `()` + tuples of
/// [`IntoValue`] up to arity 6.
pub trait IntoLuaArgs {
    /// Encode `self` (a tuple of [`IntoValue`] implementors) as a flat
    /// argument list ready for [`LuaFunction::call`].
    fn into_lua_args(self, vm: &mut Vm) -> Vec<Value>;
}

impl IntoLuaArgs for () {
    fn into_lua_args(self, _vm: &mut Vm) -> Vec<Value> {
        Vec::new()
    }
}

macro_rules! impl_into_lua_args_tuple {
    ( $( ($($name:ident: $idx:tt),+) ),+ $(,)? ) => {
        $(
            impl<$($name: IntoValue),+> IntoLuaArgs for ($($name,)+) {
                fn into_lua_args(self, vm: &mut Vm) -> Vec<Value> {
                    vec![ $( self.$idx.into_value(vm), )+ ]
                }
            }
        )+
    };
}
impl_into_lua_args_tuple! {
    (T0: 0),
    (T0: 0, T1: 1),
    (T0: 0, T1: 1, T2: 2),
    (T0: 0, T1: 1, T2: 2, T3: 3),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5),
}
