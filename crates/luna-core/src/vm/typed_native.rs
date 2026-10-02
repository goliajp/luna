//! `Vm::native_typed` + supporting traits.
//!
//! Embedders write typed Rust functions that look like Lua callables.
//! The framework decodes Lua arguments via [`FromLuaArgs`] (built on
//! per-argument [`FromLuaValue`]), invokes the typed fn, then encodes
//! the return via [`IntoLuaReturn`].
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//! use luna_core::runtime::Value;
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
//! let add = vm.native_typed(|a: i64, b: i64| -> i64 { a + b });
//! vm.set_global("add", add).unwrap();
//! let r = vm.eval("return add(40, 2)").unwrap();
//! assert!(matches!(r[0], Value::Int(42)));
//! ```
//!
//! ## Supported shapes
//!
//! - **Argument count**: 0 to 3 (4-6 land in a follow-on commit; the
//!   trampoline pattern is mechanical to extend).
//! - **Argument types**: any [`FromLuaValue`] impl — `i64`, `f64`,
//!   `bool`, `String`, `Vec<u8>`, `Value`, `Option<T>`.
//! - **Return**: any [`IntoLuaReturn`] impl — `()`, single value,
//!   tuple up to 6, or `Result<T, LuaError>` for fallible natives.
//!
//! `F` must be `Fn(...) -> Out + Copy + 'static`. Both fn pointers
//! and **non-capturing closures** qualify (the latter are ZST so we
//! reconstruct them in the trampoline). Capturing closures are not
//! supported; embedders use `vm.native_with(...)` directly with
//! explicit upvals, or a `LuaUserdata` type.

use crate::runtime::value::Value;
use crate::vm::exec::Vm;

mod decode;
mod encode;
mod sig;

pub use decode::{FromLuaArgs, FromLuaValue};
pub use encode::IntoLuaReturn;
pub use sig::{Arity0, Arity1, Arity2, Arity3, Arity4, Arity5, Arity6, NativeTypedSig};

// ─────────────────────────────────────────────────────────────────────
// Vm::native_typed
// ─────────────────────────────────────────────────────────────────────

impl Vm {
    /// Register a typed Rust function as a Lua-callable `Value`. The
    /// callable must be a fn-pointer or a non-capturing closure
    /// (`Copy + 'static + ZST or fn-pointer-sized`). For capturing
    /// closures use `vm.native_with(...)` with explicit upvals.
    ///
    /// ```
    /// # use luna_core::vm::Vm;
    /// # use luna_core::version::LuaVersion;
    /// # use luna_core::runtime::Value;
    /// # let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    /// let add = vm.native_typed(|a: i64, b: i64| -> i64 { a + b });
    /// vm.set_global("add", add).unwrap();
    /// let r = vm.eval("return add(40, 2)").unwrap();
    /// assert!(matches!(r[0], Value::Int(42)));
    /// ```
    pub fn native_typed<F, Marker>(&mut self, f: F) -> Value
    where
        F: NativeTypedSig<Marker>,
    {
        let (raw_fn, upvals) = f.into_native();
        self.native_with(raw_fn, upvals)
    }
}
