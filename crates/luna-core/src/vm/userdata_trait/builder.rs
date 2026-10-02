//! `MetatableBuilder<T>`: the concrete `UserdataMethods<T>` impl.

use std::marker::PhantomData;

use super::trampoline::{
    index_trampoline, newindex_trampoline, pack_function, pack_method, pack_method_mut,
};
use super::{LuaUserdata, MetaMethod, UserdataMethods};
use crate::runtime::heap::Gc;
use crate::runtime::table::Table;
use crate::runtime::value::{NativeFn, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::typed_native::{FromLuaArgs, IntoLuaReturn};

/// Concrete builder that emits a [`Gc<Table>`] metatable for `T`.
/// Created internally by [`Vm::register_userdata`]; embedders never
/// name this type.
pub struct MetatableBuilder<'vm, T> {
    vm: &'vm mut Vm,
    /// `__index` sub-table entries (regular methods).
    methods: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    /// Field getters for true field-style `obj.name`.
    fields_get: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    /// Field setters for `obj.name = value`.
    fields_set: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    /// Direct metatable entries (metamethods + static functions).
    meta_entries: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    _phantom: PhantomData<fn() -> T>,
}

impl<'vm, T: LuaUserdata> MetatableBuilder<'vm, T> {
    pub(super) fn new(vm: &'vm mut Vm) -> Self {
        Self {
            vm,
            methods: Vec::new(),
            fields_get: Vec::new(),
            fields_set: Vec::new(),
            meta_entries: Vec::new(),
            _phantom: PhantomData,
        }
    }

    fn intern(&mut self, s: &str) -> Gc<crate::runtime::LuaStr> {
        self.vm.heap.intern(s.as_bytes())
    }

    fn make_native(&mut self, f: NativeFn, upvals: Box<[Value]>) -> Value {
        self.vm.native_with(f, upvals)
    }

    /// Build the metatable from the accumulated entries. Called by
    /// [`Vm::register_userdata`] after [`LuaUserdata::add_methods`] returns.
    ///
    /// Three-way fork on `__index`:
    /// 1. **No methods, no field getters** → no `__index` slot.
    /// 2. **Methods only, no field getters** → fast path:
    ///    `__index` is a plain `Value::Table` of methods.
    /// 3. **Any field getters registered** → `__index` is a native
    ///    trampoline ([`index_trampoline`]) with upvals
    ///    `(methods_table_or_nil, fields_get_table)` dispatching
    ///    *methods → field-getters → nil*.
    ///
    /// `__newindex` is installed only when any field setter is
    /// registered.
    pub(super) fn finalize(self) -> Result<Gc<Table>, LuaError> {
        let MetatableBuilder {
            vm,
            methods,
            fields_get,
            fields_set,
            meta_entries,
            ..
        } = self;

        let mt = vm.heap.new_table();
        // __name — drives PUC-style error messages.
        let name_key = vm.heap.intern(b"__name");
        let type_name_str = vm.heap.intern(T::type_name().as_bytes());
        let name_val = Value::Str(type_name_str);
        // SAFETY: mt is a fresh Gc<Table>; the heap is single-threaded.
        unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(name_key), name_val)?;

        // Helper: build a Gc<Table> from a (key, value) bucket (or None
        // for the empty case so the caller can skip the allocation).
        let mk_bucket = |vm: &mut Vm,
                         entries: Vec<(Gc<crate::runtime::LuaStr>, Value)>|
         -> Result<Option<Gc<Table>>, LuaError> {
            if entries.is_empty() {
                return Ok(None);
            }
            let t = vm.heap.new_table();
            for (k, v) in entries {
                // SAFETY: t is freshly allocated.
                unsafe { t.as_mut() }.set(&mut vm.heap, Value::Str(k), v)?;
            }
            Ok(Some(t))
        };

        // __index — fork on whether any field getters are registered.
        if fields_get.is_empty() {
            // Methods-only fast path.
            if let Some(idx) = mk_bucket(vm, methods)? {
                let key = vm.heap.intern(b"__index");
                // SAFETY: mt is freshly allocated.
                unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), Value::Table(idx))?;
            }
        } else {
            // Trampoline path — methods table + fields_get table as upvals.
            let methods_val = match mk_bucket(vm, methods)? {
                Some(t) => Value::Table(t),
                None => Value::Nil,
            };
            let fields_val =
                Value::Table(mk_bucket(vm, fields_get)?.expect("fields_get non-empty checked"));
            let upvals: Box<[Value]> = Box::new([methods_val, fields_val]);
            let trampoline = vm.native_with(index_trampoline, upvals);
            let key = vm.heap.intern(b"__index");
            // SAFETY: mt is freshly allocated.
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), trampoline)?;
        }

        // __newindex — installed only when any field setter is registered.
        if !fields_set.is_empty() {
            let setters_tbl = mk_bucket(vm, fields_set)?.expect("fields_set non-empty checked");
            let upvals: Box<[Value]> =
                Box::new([Value::Table(setters_tbl), Value::Str(type_name_str)]);
            let trampoline = vm.native_with(newindex_trampoline, upvals);
            let key = vm.heap.intern(b"__newindex");
            // SAFETY: mt is freshly allocated.
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), trampoline)?;
        }

        // Direct metatable entries (metamethods + static fns).
        for (k, v) in meta_entries {
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(k), v)?;
        }
        vm.heap
            .barrier_back(mt.as_ptr() as *mut crate::runtime::heap::GcHeader);

        Ok(mt)
    }
}

impl<'vm, T: LuaUserdata> UserdataMethods<T> for MetatableBuilder<'vm, T> {
    fn add_method<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.methods.push((k, v));
    }

    fn add_method_mut<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method_mut::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.methods.push((k, v));
    }

    fn add_function<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_function::<F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.meta_entries.push((k, v));
    }

    fn add_meta_method<F, A, R>(&mut self, meta: MetaMethod, f: F)
    where
        F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(meta.name());
        self.meta_entries.push((k, v));
    }

    fn add_meta_method_mut<F, A, R>(&mut self, meta: MetaMethod, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method_mut::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(meta.name());
        self.meta_entries.push((k, v));
    }

    fn add_field_method_get<F, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T) -> Result<R, LuaError> + Copy + 'static,
        R: IntoLuaReturn + 'static,
    {
        // Adapt to add_method's (this, args) shape with A = ().
        let adapter = move |vm: &mut Vm, this: &T, _args: ()| f(vm, this);
        // The getter lives ONLY in the fields_get bucket. The
        // `__index` trampoline calls it with `(self,)` so `obj.name`
        // returns the field value directly.
        //
        // The call-syntax shape
        // (`obj:name()`) does not work for getters defined this way
        // — the trampoline calls the getter and returns its value, so
        // `obj.name` is `Value::Int(...)` not the closure, and
        // `obj:name()` evaluates to `Int(...)(obj)` which errors.
        // Embedders who need both shapes should register an explicit
        // `add_method("name", ...)` (returns the closure unchanged
        // through the table-`__index` fallback) alongside the
        // `add_field_method_get` if a same-named field-getter is also
        // wanted.
        let (raw_fn, upvals) = pack_method::<T, _, (), R>(adapter);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.fields_get.push((k, v));
    }

    fn add_field_method_set<F, A>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<(), LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
    {
        // Same trampoline shape as add_method_mut — `()` is a valid
        // `IntoLuaReturn`. Native is bucketed into `fields_set`, which
        // `newindex_trampoline` forwards to.
        let (raw_fn, upvals) = pack_method_mut::<T, F, A, ()>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.fields_set.push((k, v));
    }
}
