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
        let cl = match self.load(src.as_bytes(), name.as_bytes()) {
            Ok(c) => c,
            Err(syntax) => {
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
}
