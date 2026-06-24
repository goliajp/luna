//! Embedder ergonomics (B2, B7 — Phase 2 P2-A).
//!
//! `vm.eval` / `vm.eval_chunk` collapse the
//! `load(src.as_bytes(), name.as_bytes())? → call_value(Value::Closure(cl), &[])`
//! sequence into a single call. `vm.intern_str` exposes the heap-side
//! string interner for embedders that need a `Gc<LuaStr>` handle
//! (table key, set comparison, etc.).
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().open_math().build();
//! let r = vm.eval("return 1 + 2").unwrap();
//! assert_eq!(r.len(), 1);
//! ```

use crate::runtime::heap::Gc;
use crate::runtime::string::LuaStr;
use crate::runtime::value::Value;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Vm {
    /// Same as [`Vm::eval`] but with a user-supplied chunk name
    /// (appears in tracebacks for debugging).
    pub fn eval_chunk(&mut self, src: &str, name: &str) -> Result<Vec<Value>, LuaError> {
        self.clear_error_metadata();
        let cl = match self.load(src.as_bytes(), name.as_bytes()) {
            Ok(c) => c,
            Err(syntax) => {
                // B6: classify + record source position.
                self.set_error_kind(crate::vm::error::LuaErrorKind::Syntax);
                self.set_error_source(name.to_string(), syntax.line);
                // Surface SyntaxError as a LuaError carrying the
                // formatted PUC-style message (`<line>: <msg>`).
                let msg = format!("{}", syntax);
                let s = self.heap.intern(msg.as_bytes());
                return Err(LuaError(Value::Str(s)));
            }
        };
        self.call_value(Value::Closure(cl), &[])
    }

    /// Intern a UTF-8 string into the heap's string table.
    /// Idempotent — interning the same bytes twice returns the same
    /// [`Gc<LuaStr>`] handle.
    ///
    /// Useful for embedders constructing table keys or comparing Lua
    /// strings without going through `Value::Str` wrapping each time.
    pub fn intern_str(&mut self, s: &str) -> Gc<LuaStr> {
        self.heap.intern(s.as_bytes())
    }

    // ─── B12 host-root pool — moved to `crate::vm::host_roots` ───
    //
    // v1.3 Phase SR migrated the append-only `Vec<Value>` to a
    // slot-recycling pool keyed by `HostRootTicket { idx, generation }`.
    // The new API surface (`pin_host` / `read_host` / `write_host` /
    // `unpin` / `unpin_all` / `host_root_count`) lives in
    // [`crate::vm::host_roots`]; the type re-exports are in
    // [`crate::vm`] (`HostRootTicket`, `HostRootStale`).
    //
    // Breaking change vs v1.2 / v1.1: `pin_host` returns
    // `HostRootTicket` (was `usize`); `host_root_at` / `host_root_set`
    // are removed in favor of `read_host` / `write_host` which
    // validate the ticket's generation. See CHANGELOG `[1.3.0]`
    // Phase SR section for the migration recipe.

    // ─── B6 LuaError classification ──────────────────────────────
    //
    // The error value itself (`LuaError(pub Value)`) stays `Copy` so
    // the 379 existing references / 34 construction sites compile
    // unchanged. Richer context lives on the Vm; embedders read it
    // via these accessors after observing a `Result::Err(LuaError)`.

    /// Classification of the most recently raised error on this Vm.
    /// Returns [`crate::vm::error::LuaErrorKind::Runtime`] before any error fires.
    pub fn error_kind(&self) -> crate::vm::error::LuaErrorKind {
        self.last_error_kind
    }

    /// `(source_name, line)` of the most recently raised error, or
    /// `None` if the dispatcher could not locate one. Source names
    /// match Lua's chunk-name convention (`"=eval"`, `"=stdin"`,
    /// user-supplied via `Vm::load`).
    pub fn error_source(&self) -> Option<(&str, u32)> {
        self.last_error_source
            .as_ref()
            .map(|(s, l)| (s.as_str(), *l))
    }

    /// Set the classification for the next error to be raised — used
    /// by the dispatcher at well-known sites. Embedders writing
    /// native callbacks may call this before returning `Err(LuaError)`
    /// to flag a specific kind (e.g. `LuaErrorKind::Type` for a bad
    /// arg).
    pub fn set_error_kind(&mut self, kind: crate::vm::error::LuaErrorKind) {
        self.last_error_kind = kind;
    }

    /// Set the `(source_name, line)` for the next error to be raised.
    /// The dispatcher uses this at the syntax-error / parser
    /// boundary.
    pub fn set_error_source(&mut self, name: String, line: u32) {
        self.last_error_source = Some((name, line));
    }

    /// Clear error classification — called on a clean `call_value`
    /// entry so old error metadata doesn't leak into the next call.
    pub fn clear_error_metadata(&mut self) {
        self.last_error_kind = crate::vm::error::LuaErrorKind::default();
        self.last_error_source = None;
    }

    // ─── B8 LuaUserdata host payloads ────────────────────────────
    //
    // The closed-world userdata GC infrastructure (`Gc<Userdata>` +
    // metatable + `__gc`) is already in place; B8 just unlocks the
    // `Host { type_id, data: Box<dyn Any> }` payload variant for
    // embedders to stash arbitrary `T: 'static` Rust values.
    //
    // v1.1 restricts host types to `'static` (typically heap-only
    // `Box<...>` or `Rc<...>` to non-Gc objects). Trace-bearing host
    // payloads land in Phase 4+ alongside the userdata Trace ripple.
    //
    // v1.2 Track B: bounds tightened from `T: Any + 'static` to
    // `T: LuaUserdata` so the metatable produced by `T::add_methods`
    // is auto-installed at `create_userdata` time. Source-compatible
    // for B8 users via a one-line `impl LuaUserdata for T {}`.

    /// Allocate a host userdata wrapping `value`. Returns the
    /// `Value::Userdata` you can `set_global` / pin / pass to scripts.
    ///
    /// The metatable produced by [`crate::vm::LuaUserdata::add_methods`]
    /// is auto-installed on the userdata (cached per `Vm` keyed by
    /// `TypeId::of::<T>()`). For a type that only needs identity +
    /// raw host-side access (no Lua-callable methods), provide an
    /// empty impl:
    ///
    /// ```
    /// # use luna_core::vm::LuaUserdata;
    /// struct Counter(i64);
    /// impl LuaUserdata for Counter {}
    /// ```
    ///
    /// ```
    /// use luna_core::vm::{LuaUserdata, Vm};
    /// use luna_core::version::LuaVersion;
    /// use luna_core::runtime::Value;
    ///
    /// #[derive(Debug)]
    /// struct Counter(i64);
    /// impl LuaUserdata for Counter {}
    ///
    /// let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    /// let ud = vm.create_userdata(Counter(42));
    /// vm.set_global("counter", ud).unwrap();
    ///
    /// match ud {
    ///     Value::Userdata(g) => {
    ///         // SAFETY: single-threaded heap; pointer is live.
    ///         let r = unsafe { &*g.as_ptr() };
    ///         assert_eq!(r.downcast::<Counter>().unwrap().0, 42);
    ///     }
    ///     _ => unreachable!(),
    /// }
    /// ```
    pub fn create_userdata<T: crate::vm::LuaUserdata>(&mut self, value: T) -> Value {
        // Phase TB (v1.3): capture a monomorphic trace adapter for `T`.
        // The fn item `trace_fn_for::<T>` is a distinct code address
        // per `T` (LLVM monomorphization); the downcast cannot fail
        // because `register_userdata::<T>` pairs the adapter with
        // `TypeId::of::<T>()` here, and `Userdata::trace` always reads
        // the adapter back through the same `Host` instance.
        fn trace_fn_for<T: crate::vm::LuaUserdata>(
            any: &(dyn std::any::Any + 'static),
            m: &mut crate::vm::UserdataMarker<'_>,
        ) {
            let typed = any
                .downcast_ref::<T>()
                .expect("LuaUserdata trace adapter / TypeId mismatch");
            typed.trace(m);
        }
        let payload = crate::runtime::userdata::UserdataPayload::Host {
            type_id: std::any::TypeId::of::<T>(),
            data: Box::new(value),
            trace_fn: Some(trace_fn_for::<T>),
        };
        let g = self.heap.new_userdata(payload, /* writable */ true);
        // v1.2 Track B — install the trait-derived metatable (or
        // fetch the cached one). Build only fails if the metatable's
        // table set overflows MAX_ASIZE, which is impossible with
        // <100 entries; expect-on-fail is appropriate here.
        let mt = self
            .register_userdata::<T>()
            .expect("LuaUserdata metatable build overflowed");
        // SAFETY: g is a freshly allocated Gc<Userdata>; the heap is
        // single-threaded and the pointer is live.
        unsafe { g.as_mut() }.set_metatable(Some(mt));
        self.heap
            .barrier_back(g.as_ptr() as *mut crate::runtime::heap::GcHeader);
        // PUC contract: __gc is registered for finalization at
        // metatable-set time, not at later mutation of the metatable.
        self.check_finalizer_userdata(g);
        Value::Userdata(g)
    }

    /// Convenience: [`Self::create_userdata`] + [`Self::set_global`].
    pub fn set_userdata<T: crate::vm::LuaUserdata>(
        &mut self,
        name: &str,
        value: T,
    ) -> Result<(), LuaError> {
        let ud = self.create_userdata(value);
        self.set_global(name, ud)
    }

    /// Borrow the host payload of a global userdata as `&T`. Returns
    /// `None` if the global doesn't exist, isn't a userdata, isn't a
    /// host userdata, or holds a different type than `T`.
    ///
    /// Takes `&mut self` because the lookup interns the key string;
    /// returning a borrow tied to `&mut Vm` mirrors `vm.set_global`
    /// ergonomics.
    pub fn userdata_borrow<T: std::any::Any + 'static>(&mut self, name: &str) -> Option<&T> {
        let key = Value::Str(self.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> = NonNull<T> over the single-threaded GC heap.
        let v = unsafe { (*self.globals().as_ptr()).get(key) };
        match v {
            Value::Userdata(g) => {
                // SAFETY: single-threaded GC heap; the Gc<Userdata>
                // stays live as long as it's reachable from globals.
                let ud = unsafe { &*g.as_ptr() };
                ud.downcast::<T>()
            }
            _ => None,
        }
    }

    // ─── B9 Rust-side coroutine drive ────────────────────────────

    /// Create a new coroutine carrying `body` (a Lua function or
    /// any callable Value). Returns the `Value::Coro` handle ready
    /// to be passed to [`Self::resume_coroutine`].
    ///
    /// Equivalent to `coroutine.create(body)` from a Rust embedder.
    pub fn create_coroutine(&mut self, body: Value) -> Value {
        let co = self.new_coro(body);
        Value::Coro(co)
    }

    /// Resume a coroutine with the given arguments. Returns the
    /// yielded values on `yield`, the return values on the body's
    /// terminal `return`, or an error if the body raised.
    ///
    /// Equivalent to `coroutine.resume(co, args...)`. Returns
    /// `Err(LuaError)` if `co` is not a `Value::Coro`.
    pub fn resume_coroutine(
        &mut self,
        co: Value,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, LuaError> {
        let coro = match co {
            Value::Coro(c) => c,
            _ => return Err(LuaError(Value::Nil)),
        };
        self.resume_coro(coro, args)
    }

    // ─── B11 Rust-side debug hook ────────────────────────────────

    /// Install a Rust-side debug hook (see [`crate::vm::exec::RustDebugHook`]). The
    /// `mask` is a bitwise OR of `HOOK_MASK_CALL` / `HOOK_MASK_RETURN`
    /// / `HOOK_MASK_LINE` / `HOOK_MASK_COUNT` exported from
    /// [`crate::vm::exec`]. The `count` arg sets the instruction
    /// granularity for `Count` events (ignored unless `HOOK_MASK_COUNT`
    /// is set).
    ///
    /// Passing `hook = None` clears the Rust hook; the Lua-side hook
    /// installed via `debug.sethook` is unaffected.
    pub fn set_rust_debug_hook(
        &mut self,
        hook: Option<crate::vm::exec::RustDebugHook>,
        mask: u32,
        count: i64,
    ) {
        self.hook.rust_func = hook;
        // Update event mask flags. Other categories of the Lua hook
        // stay as they were so a Lua-side debug.sethook + Rust hook
        // can coexist with independent event subscriptions.
        if hook.is_some() {
            self.hook.call |= mask & crate::vm::exec::HOOK_MASK_CALL != 0;
            self.hook.ret |= mask & crate::vm::exec::HOOK_MASK_RETURN != 0;
            self.hook.line |= mask & crate::vm::exec::HOOK_MASK_LINE != 0;
            if mask & crate::vm::exec::HOOK_MASK_COUNT != 0 {
                self.hook.count = true;
                self.hook.count_base = count;
                self.hook.count_left = count;
            }
        }
    }

    /// Clear the Rust-side debug hook (sugar over
    /// `set_rust_debug_hook(None, 0, 0)`).
    pub fn clear_rust_debug_hook(&mut self) {
        self.hook.rust_func = None;
    }

    /// Read the most recently dispatched Lua opcode, if the Vm is currently
    /// executing inside a Lua frame. Intended for use from a Count hook
    /// (installed via [`Self::set_rust_debug_hook`] with `HOOK_MASK_COUNT`)
    /// to tally per-opcode distribution against a workload — the v1.2
    /// methodology gate (`perf-decomposition-vs-polish.md` §2 Phase A,
    /// in `~/.claude-shared/global/methodology/`) requires runtime-counter
    /// validation of per-iter op mix before any stage decomposition is
    /// acted on.
    ///
    /// Returns `None` outside a Lua frame (top-level setup, while a
    /// native callback or Cont guard is on top of the call stack, etc.).
    /// Reads `self.frames.last() → CallFrame::Lua(f) → f.closure.proto.code[f.pc - 1]`
    /// — the just-dispatched opcode (PC has already advanced past it).
    pub fn current_op(&self) -> Option<crate::vm::isa::Op> {
        let f = self.jit_last_lua_frame()?;
        let pc = (f.pc as usize).checked_sub(1)?;
        let inst = f.closure.proto.code.get(pc)?;
        Some(inst.op())
    }

    /// Mutable variant of [`Self::userdata_borrow`].
    pub fn userdata_borrow_mut<T: std::any::Any + 'static>(
        &mut self,
        name: &str,
    ) -> Option<&mut T> {
        let key = Value::Str(self.heap.intern(name.as_bytes()));
        // SAFETY: see userdata_borrow.
        let v = unsafe { (*self.globals().as_ptr()).get(key) };
        match v {
            Value::Userdata(g) => {
                // SAFETY: see userdata_borrow; the returned &mut is
                // exclusive within the &mut self window.
                let ud = unsafe { &mut *g.as_ptr() };
                ud.downcast_mut::<T>()
            }
            _ => None,
        }
    }
}
