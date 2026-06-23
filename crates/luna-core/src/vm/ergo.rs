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

    // ─── B12 host-root pool ───────────────────────────────────────
    //
    // The `luna::Lua` facade (in the `luna` crate) leans on these
    // methods to keep `LuaFunction` / `LuaTable` / `LuaRoot` handles
    // alive across calls. The pool is append-only in v1.1; slot
    // recycling lands in Phase 3 alongside B8 LuaUserdata.

    /// Pin `v` as a host root and return its slot index. The value
    /// becomes an extra GC root until the index is reset via
    /// [`unpin_all`](Self::unpin_all).
    pub fn pin_host(&mut self, v: Value) -> usize {
        self.host_roots.push(v);
        self.host_roots.len() - 1
    }

    /// Read a previously pinned host root. Panics if `idx` was never
    /// pinned (or if the pool was cleared by `unpin_all`).
    pub fn host_root_at(&self, idx: usize) -> Value {
        self.host_roots[idx]
    }

    /// Mutate a previously pinned host root (for the `Lua` facade's
    /// `LuaTable::set` after an in-place rewrite). Panics on OOB.
    pub fn host_root_set(&mut self, idx: usize, v: Value) {
        self.host_roots[idx] = v;
    }

    /// Number of currently-pinned host roots.
    pub fn host_root_count(&self) -> usize {
        self.host_roots.len()
    }

    /// Drop every pinned host root. Embedders driving the `Lua`
    /// facade in a request-per-script loop call this between requests
    /// to keep the pool bounded.
    pub fn unpin_all(&mut self) {
        self.host_roots.clear();
    }

    // ─── B6 LuaError classification ──────────────────────────────
    //
    // The error value itself (`LuaError(pub Value)`) stays `Copy` so
    // the 379 existing references / 34 construction sites compile
    // unchanged. Richer context lives on the Vm; embedders read it
    // via these accessors after observing a `Result::Err(LuaError)`.

    /// Classification of the most recently raised error on this Vm.
    /// Returns [`LuaErrorKind::Runtime`] before any error fires.
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

    /// Allocate a host userdata wrapping `value`. Returns the
    /// `Value::Userdata` you can `set_global` / pin / pass to scripts.
    ///
    /// ```
    /// use luna_core::vm::Vm;
    /// use luna_core::version::LuaVersion;
    /// use luna_core::runtime::Value;
    ///
    /// #[derive(Debug)]
    /// struct Counter(i64);
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
    pub fn create_userdata<T: std::any::Any + 'static>(&mut self, value: T) -> Value {
        let payload = crate::runtime::userdata::UserdataPayload::Host {
            type_id: std::any::TypeId::of::<T>(),
            data: Box::new(value),
        };
        let g = self.heap.new_userdata(payload, /* writable */ true);
        Value::Userdata(g)
    }

    /// Convenience: [`Self::create_userdata`] + [`Self::set_global`].
    pub fn set_userdata<T: std::any::Any + 'static>(
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

    // ─── B11 Rust-side debug hook ────────────────────────────────

    /// Install a Rust-side debug hook (see [`RustDebugHook`]). The
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
