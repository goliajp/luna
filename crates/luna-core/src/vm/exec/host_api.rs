//! The embedder-facing surface: globals, natives, loading and calling
//! chunks, the random generator and the macro hooks.

use super::*;
use crate::native_stack::{HANDLER_RESERVE, RESERVE, is_low};

impl Vm {
    /// xoshiro256** next.
    pub(crate) fn rng_next(&mut self) -> u64 {
        let s = &mut self.rng;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Seed the RNG via splitmix64 expansion (PUC randseed shape).
    pub(crate) fn rng_seed(&mut self, a: u64, b: u64) {
        // PUC setseed: state = [n1, 0xff, n2, 0] (0xff avoids an all-zero
        // state), then 16 discards to spread the seed. Matches PUC's exact
        // sequence so the low-level conformance test passes.
        self.rng = [a, 0xff, b, 0];
        for _ in 0..16 {
            self.rng_next();
        }
    }

    /// Wall-clock since VM creation (os.clock approximation).
    pub(crate) fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Entropy for math.randomseed() with no arguments.
    pub(crate) fn rng_auto_seed(&mut self) -> (i64, i64) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let addr = &self.rng as *const _ as u64;
        (t as i64, addr as i64)
    }

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

    /// Remaining VM stack slots (PUC `L->stack_last - L->top` analogue).
    /// Library code that pushes a known number of fresh slots — e.g.
    /// `table.unpack` returning N values — consults this to refuse when
    /// the push would blow past `LUAI_MAXSTACK`. 5.3 coroutine.lua :530's
    /// `for j in {lim-10, lim-5, …}` series pins this contract: the
    /// coroutine's already-built table eats a few slots, so an unpack of
    /// ~lim values can't fit.
    pub(crate) fn stack_room(&self) -> i64 {
        PUC_MAXSTACK - (self.stack.len() as i64)
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

    /// Call `f` with `args` in protected mode with the message handler
    /// `msgh`: PUC `lua_pcall(L, nargs, LUA_MULTRET, msgh)` made by the host.
    ///
    /// `msgh` runs where the error was raised, before the stack unwinds, so
    /// it can take a traceback of the failing call ([`Vm::traceback`]); an
    /// error inside it calls it again with the new error, as in PUC. The
    /// returned error carries what the handler returned.
    ///
    /// Like `lua_pcall`, the call is not a level of the stack: a traceback
    /// taken inside ends with `f`.
    pub fn call_value_with_handler(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        self.host_pcall(crate::vm::builtins::nat_host_xpcall, f, args, msgh)
    }

    /// [`Vm::call_value_with_handler`] made from inside a C function of the
    /// host's, as lua.c's `docall` runs inside `pmain`: that function is one
    /// C level below `f`, which `debug.getinfo` finds and a traceback ends
    /// with (`[C]: in ?`, 5.1 `[C]: ?`).
    #[doc(hidden)]
    pub fn call_value_with_handler_in_c(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        self.host_pcall(crate::vm::builtins::nat_host_xpcall_in_c, f, args, msgh)
    }

    /// `f(args)` in protected mode with no message handler, made from inside
    /// a C function of the host's: PUC `lua_pcall(L, n, r, 0)` inside a C
    /// function, as lua.c's `l_print` calls `print` from `pmain`. That
    /// function is one C level below `f`, as for
    /// [`Vm::call_value_with_handler_in_c`].
    #[doc(hidden)]
    pub fn call_value_in_c(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        let level = self.native(crate::vm::builtins::nat_host_pcall_in_c);
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(f);
        call_args.extend_from_slice(args);
        let mut results = self.call_value(level, &call_args)?;
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
            Err(LuaError(results.get(1).copied().unwrap_or(Value::Nil)))
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

    /// [`Vm::call_value_with_handler`] that also says how it failed: `true`
    /// when the handler itself failed and the error is "error in error
    /// handling" (PUC's LUA_ERRERR), `false` for any other error
    /// (LUA_ERRRUN). For the C API's `lua_pcall`.
    #[doc(hidden)]
    pub fn call_value_with_handler_status(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, (LuaError, bool)> {
        let before = self.errerr_raised;
        self.call_value_with_handler(f, args, msgh).map_err(|e| {
            // a handler may return the same text itself; only an error the
            // vm turned into it during this call is LUA_ERRERR
            let errerr = self.errerr_raised != before
                && matches!(e.0, Value::Str(s) if s.as_bytes() == b"error in error handling");
            (e, errerr)
        })
    }

    fn host_pcall(
        &mut self,
        level: crate::runtime::value::NativeFn,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        let level = self.native(level);
        let mut call_args = Vec::with_capacity(args.len() + 2);
        call_args.push(f);
        call_args.push(msgh);
        call_args.extend_from_slice(args);
        let mut results = self.call_value(level, &call_args)?;
        // the protected call's `true, results...` or `false, handled error`
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
            Err(LuaError(results.get(1).copied().unwrap_or(Value::Nil)))
        }
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

    /// Call any callable value from the host (or from natives like pcall).
    pub fn call_value(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        // host-level entry (no enclosing exec): drop any error state from a
        // prior call that propagated uncaught (`error_traceback` would
        // otherwise leak into the next debug.traceback call).
        if self.public_call_depth == 0 {
            self.error_traceback = None;
        }
        self.public_call_depth += 1;
        // JIT fast path. A host call with no args targeting a Lua
        // chunk whose body fits the int-arith whitelist short-circuits
        // the whole interpreter dispatch and runs straight through the
        // mmap'd native code. The lookup is one Cell::get + one match —
        // the slow path (compile attempt on first reach) is paid once per
        // Proto.
        let r = match f {
            Value::Closure(cl) if args.is_empty() => self.try_jit_call(cl),
            _ => None,
        };
        let r = match r {
            Some(Ok(vs)) => {
                self.public_call_depth -= 1;
                return Ok(vs);
            }
            Some(Err(e)) => Err(e),
            None => self.call_value_impl(f, args, true, None),
        };
        if let Err(e) = r
            && self.public_call_depth == 1
            && self.current.is_none()
        {
            self.raise_native_to_host(e.0);
        }
        self.public_call_depth -= 1;
        r
    }

    /// `call_value` with control over the `from_c` debug boundary. A `__close`
    /// handler runs *within* the closing Lua frame's activation (PUC luaF_close
    /// invokes it inside that ci), so it is called with `from_c = false`: its
    /// debug parent is the closing function, not a synthetic C level.
    /// `at`: the slot to call at, PUC's `L->top` where a message handler
    /// runs (see `raise_top`); the slots above it belong to the frame
    /// that raised, dead past that top as PUC's are. Else the stack's end.
    pub(crate) fn call_value_impl(
        &mut self,
        f: Value,
        args: &[Value],
        from_c: bool,
        at: Option<u32>,
    ) -> Result<Vec<Value>, LuaError> {
        // the native stack too (a first level is no nesting: unchecked);
        // a message handler running on the error gets half the reserve
        if self.g.nccalls > 0 && is_low(RESERVE) {
            if self.msgh_depth == 0 {
                return Err(self.runerror("C stack overflow"));
            }
            if is_low(HANDLER_RESERVE) {
                return Err(LuaError(self.errerr()));
            }
        }
        self.check_c_level(true)?;
        self.g.nccalls += 1;
        let len = self.stack.len();
        let func_slot = match at {
            None => {
                self.stack.push_or_abort(f);
                self.stack.extend_from_slice_or_abort(args);
                self.top = self.stack.len() as u32;
                len as u32
            }
            Some(slot) => {
                self.place_call(slot, f, args);
                self.top = self.stack.len().max(slot as usize + 1 + args.len()) as u32;
                slot
            }
        };
        let r = self.call_at(func_slot, args.len() as u32, from_c);
        self.g.nccalls -= 1;
        // a call placed inside a frame's window gives the window back: the
        // frames below run on when the error is caught
        if at.is_some() && self.stack.len() < len {
            self.grow_stack_or_abort(len);
        }
        if r.is_err()
            && self.yielding.is_none()
            && self.terminating.is_none()
            && !self.host_yield_pending
            && self.pending_async_native_fut.is_none()
        {
            // A `coroutine.yield` in flight raises a sentinel error to unwind the
            // Rust stack, but the suspended coroutine's frames/registers (which
            // sit at/above `func_slot`) must survive for the next resume — so we
            // only truncate on a real error. A self-close termination is in the
            // same boat: the dying thread's state is discarded wholesale.
            // A `host_yield_pending` cooperative yield is in
            // the same boat as `yielding`: the next `EvalFuture::poll`
            // resumes the same call, so the in-flight frames must
            // survive.
            self.stack.truncate(func_slot as usize);
            self.top = func_slot;
        }
        r
    }
}
