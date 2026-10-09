//! The embedder-facing surface: globals, natives, loading and calling
//! chunks, and the macro hooks.

use super::*;
use crate::native_stack::{HANDLER_RESERVE, RESERVE, is_low};
mod call_value;

impl Vm {
    /// Allocate a native function object (no upvalues): builtin registration.
    pub fn native(&mut self, f: crate::runtime::value::NativeFn) -> Value {
        Value::Native(self.heap.new_native(f, Box::new([])))
    }

    /// Allocate a native function object with captured upvalues.
    pub fn native_with(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Value {
        Value::Native(self.heap.new_native(f, upvals))
    }

    /// Install the shared string metatable (string library).
    pub fn set_string_metatable(&mut self, mt: Option<Gc<Table>>) {
        self.type_mt[3] = mt;
    }

    /// The current globals table (`_G` / `_ENV` source for new chunks).
    pub fn globals(&self) -> Gc<Table> {
        self.globals
    }

    /// PUC `lua_checkstack(L, n)` (5.2+) with the thread's top at slot
    /// `top`: whether `n` more slots fit, by the limit a call meets (see
    /// `lua_stack_limit`). It counts from the live top, not from how far
    /// the stack has ever grown. A stack in its error space (an overflow
    /// is being handled) has that space too. 5.4+ grow a stack refused
    /// this way to that size, as an overflow does.
    pub(crate) fn checkstack(&mut self, top: u32, n: i64) -> bool {
        let room = if self.stack_extra {
            STACK_ERR_SPACE - 1
        } else {
            0
        };
        let fits = i64::from(top) + n <= i64::from(self.g.lua_stack_limit + room);
        if !fits && self.version() >= LuaVersion::Lua54 {
            self.stack_extra = true;
        }
        fits
    }

    /// Repoint the thread's "global table" used by *future* `Vm::load` calls
    /// for the chunk's `_ENV` upvalue (PUC 5.1 `setfenv(0, env)` rewrites
    /// `L->l_gt`). Already-loaded chunks keep their own snapshot via the
    /// per-closure cell-0 clone in `Op::Closure`, so they are unaffected.
    pub(crate) fn set_globals(&mut self, env: Gc<Table>) {
        self.globals = env;
    }

    /// The Lua dialect this VM was constructed for (5.1 / 5.2 / 5.3 / 5.4 /
    /// 5.5). Determines numeric semantics, available standard libraries, and
    /// metamethod behavior.
    pub fn version(&self) -> LuaVersion {
        self.version
    }

    /// Set a global by name. `v` may be any `IntoValue`: a primitive
    /// (`i64`, `f64`, `bool`, `&str`, `String`, `Vec<u8>`), a `Value`
    /// directly, an `Option<T>`, or a `Gc<Table>` / `Gc<LuaClosure>` /
    /// `Gc<NativeClosure>` handle.
    ///
    /// Returns `Err(LuaError)` if the globals table is read-only (see
    /// [`Vm::set_readonly`]) or overflows (extremely unlikely in practice
    /// — `MAX_ASIZE = 1 << 27`). String interning + key construction
    /// cannot fail.
    ///
    /// ```
    /// # use luna_core::vm::Vm;
    /// # use luna_core::version::LuaVersion;
    /// let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    /// vm.set_global("answer", 42).unwrap();
    /// vm.set_global("ratio", 0.5_f64).unwrap();
    /// vm.set_global("hello", "world").unwrap();
    /// let r = vm.eval("return answer, ratio, hello").unwrap();
    /// assert_eq!(r.len(), 3);
    /// ```
    pub fn set_global<V: crate::vm::IntoValue>(
        &mut self,
        name: &str,
        v: V,
    ) -> Result<(), LuaError> {
        self.set_global_bytes(name.as_bytes(), v)
    }

    /// [`Vm::set_global`] for a name that is not UTF-8: Lua strings are
    /// bytes, and a program's command line or environment may hand over
    /// any.
    pub fn set_global_bytes<V: crate::vm::IntoValue>(
        &mut self,
        name: &[u8],
        v: V,
    ) -> Result<(), LuaError> {
        let v = v.into_value(self);
        let k = Value::Str(self.heap.intern(name));
        // SAFETY: `self.globals` is a root of this Vm; the borrow lives for the one `set`, which touches only the heap and the table and does not collect, and `&mut self` rules out another reference into it
        if let Err(e) = unsafe { self.globals.as_mut() }.set(&mut self.heap, k, v) {
            return Err(self.table_error(e));
        }
        self.heap.barrier_back(self.globals);
        Ok(())
    }

    /// Mark `t` read-only (`on = true`) or writable again (`on = false`),
    /// as Redis's `lua_enablereadonlytable` does for the tables of its
    /// scripting environment. While `t` is read-only every write to it
    /// raises "Attempt to modify a readonly table", in every dialect and
    /// with or without the JIT: assignments (`t.k = v`, `t[k] = v`, a
    /// global assignment when `t` is the globals table, a `__newindex`
    /// chain that reaches `t`), `rawset`, `setmetatable` and
    /// `debug.setmetatable`, and the table library's stores
    /// (`table.insert`, `table.remove`, `table.sort`, `table.move`'s
    /// destination). [`Vm::set_global`] and [`Table::set`] refuse it too.
    /// An assignment's error carries the position of the Lua code that
    /// made it (`user_script:1: Attempt to modify a readonly table`); one
    /// raised inside a library function carries none. Reads, including
    /// `__index` lookups, cost nothing extra. A host that has to change a
    /// read-only table unmarks it, writes, and marks it again.
    ///
    /// ```
    /// # use luna_core::vm::Vm;
    /// # use luna_core::version::LuaVersion;
    /// let mut vm = Vm::sandbox(LuaVersion::Lua51).open_base().open_string().build();
    /// let string_lib = match vm.eval("return string").unwrap()[0] {
    ///     luna_core::runtime::Value::Table(t) => t,
    ///     _ => unreachable!(),
    /// };
    /// vm.set_readonly(string_lib, true);
    /// let e = vm.eval("string.foo = 1").unwrap_err();
    /// assert!(vm.error_text(&e).ends_with("Attempt to modify a readonly table"));
    /// vm.set_readonly(string_lib, false);
    /// vm.eval("string.foo = 1").unwrap();
    /// ```
    pub fn set_readonly(&mut self, t: Gc<Table>, on: bool) {
        // SAFETY: a `Gc` handle points at a live table (see `Gc`); the
        // borrow lives for this one flag update, which reaches no other
        // reference into the table
        unsafe { t.as_mut() }.set_readonly(on);
    }

    /// Backward write barrier shorthand for native lib code: demote `t` from
    /// BLACK back to gray so the next propagate step re-traces its fields.
    /// No-op outside Propagate (parent is never BLACK at mutation time).
    pub(crate) fn barrier_back_table(&mut self, t: Gc<Table>) {
        self.heap.barrier_back(t);
    }

    /// Forward write barrier shorthand: a closed upvalue is a single-slot
    /// container — `barrier_forward` is cheaper than `barrier_back` here.
    /// No-op outside Propagate.
    pub(crate) fn barrier_forward_upvalue(&mut self, uv: Gc<Upvalue>, child: Value) {
        self.heap.barrier_forward(uv, child);
    }

    /// Register a MacroLua macro under `name`. Inert
    /// under non-MacroLua dialects (the macro is stored but the load
    /// path only consults the registry when
    /// `self.version == LuaVersion::MacroLua`).
    ///
    /// `name` is stored without the leading `@` — source code writes
    /// `@double(x)` to invoke a macro registered as `"double"`.
    pub fn define_macro(&mut self, name: &str, m: Box<dyn crate::frontend::macro_expander::Macro>) {
        self.macro_registry.register(name, m);
    }

    /// Drop all MacroLua macros (built-in + custom).
    /// Mostly useful for tests.
    pub fn clear_macros(&mut self) {
        self.macro_registry.clear();
    }

    /// PUC `luaL_loadfilex`: compile the file `name` (standard input when
    /// `None`, named `stdin`) into a function. A first line starting with
    /// `#` is skipped; `mode` (`"t"`, `"b"`, `"bt"`, `None` for both)
    /// limits the chunk to text and/or binary. The error is the message
    /// PUC's function leaves: `cannot open <name>: <reason>` when the file
    /// cannot be read, or the positioned syntax error.
    pub fn load_file(
        &mut self,
        name: Option<&[u8]>,
        mode: Option<&[u8]>,
    ) -> Result<Value, LuaError> {
        crate::vm::lib_os_io::load_path(self, name, mode).map_err(LuaError)
    }

    /// PUC `luaL_loadbufferx`: compile `src` under `chunkname`, the chunk
    /// kind limited by `mode` as in [`Vm::load_file`]. A syntax error comes
    /// back as its positioned message (`<chunk id>:<line>: <message>`), the
    /// string `load` returns.
    pub fn load_buffer(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        mode: Option<&[u8]>,
    ) -> Result<Value, LuaError> {
        crate::vm::lib_os_io::load_chunk(self, src, chunkname, mode).map_err(LuaError)
    }

    /// Compile and run `src` as an anonymous chunk; return its results.
    /// Source name in the traceback is `"=eval"`. Syntax errors are
    /// surfaced as `LuaError` carrying the formatted PUC-style message
    /// (interned through the heap so the error value composes with
    /// `pcall` / `error_text` like any runtime error).
    pub fn eval(&mut self, src: &str) -> Result<Vec<Value>, LuaError> {
        self.eval_chunk(src, "=eval")
    }

    /// Render an error value for messages/tests. Non-string errors —
    /// `error({code=…})`, `error(42)`, etc. — collapse to a type tag
    /// (`"(error object is a table value)"`); embedders that need
    /// structured payloads should inspect `e.0` directly. Errors whose
    /// text starts with `"native panic:"` indicate a Rust panic
    /// crossed `catch_unwind` — the Vm may be inconsistent and should
    /// be dropped (do not reuse).
    pub fn error_text(&self, e: &LuaError) -> String {
        match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v => format!("(error object is a {} value)", v.type_name()),
        }
    }

    /// Render an error value the way PUC's standalone `msghandler`
    /// does (lua.c): strings pass through, numbers stringify, and any
    /// other object is given a chance at its `__tostring` metamethod
    /// (the result must be a string) before collapsing to the
    /// `"(error object is a … value)"` tag. Needs `&mut self` because
    /// `__tostring` runs arbitrary Lua — `error_text` remains the
    /// non-executing variant.
    pub fn error_display(&mut self, e: &LuaError) -> String {
        match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v @ (Value::Int(_) | Value::Float(_)) => {
                String::from_utf8_lossy(&self.tostring_basic(v)).into_owned()
            }
            v => {
                let mm = self.get_mm(v, Mm::ToString);
                if !mm.is_nil()
                    && let Ok(r) = self.call_value(mm, &[v])
                    && let Some(Value::Str(s)) = r.first()
                {
                    return String::from_utf8_lossy(s.as_bytes()).into_owned();
                }
                format!("(error object is a {} value)", v.type_name())
            }
        }
    }

    /// `t[key]` with metamethods, as the Lua expression does (PUC
    /// `lua_gettable`). For the C API.
    #[doc(hidden)]
    pub fn index_with_mm(&mut self, t: Value, key: Value) -> Result<Value, LuaError> {
        self.index_value(t, key)
    }

    /// `t[key] = v` with metamethods, as the Lua assignment does (PUC
    /// `lua_settable`). For the C API.
    #[doc(hidden)]
    pub fn set_index_with_mm(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        self.newindex_value(t, key, v)
    }

    /// PUC `luaL_getmetafield`: the field `event` of `v`'s metatable, read
    /// raw; nil when `v` has no metatable or the field is absent.
    pub fn metafield(&mut self, v: Value, event: &str) -> Value {
        match self.metatable_of(v) {
            Some(mt) => {
                let key = Value::Str(self.heap.intern(event.as_bytes()));
                mt.get(key)
            }
            None => Value::Nil,
        }
    }
}
