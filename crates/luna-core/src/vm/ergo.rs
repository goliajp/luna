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
}
