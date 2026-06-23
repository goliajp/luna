//! `IntoValue` trait (B4, Phase 2 P2-B) — convert Rust primitive
//! types into Lua `Value`s through a `&mut Vm` (so string-shaped
//! conversions can intern through the heap).
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//! use luna_core::runtime::Value;
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
//! vm.set_global("answer", 42).unwrap();
//! vm.set_global("name", "luna").unwrap();
//! let r = vm.eval("return answer, name").unwrap();
//! assert_eq!(r.len(), 2);
//! ```
//!
//! Built-in implementations cover the common cases:
//! `Value` (identity), `()` (Nil), `bool`, `i64` / `i32` / `i16` /
//! `i8` / `u32` / `u16` / `u8`, `f64` / `f32`, `&str` / `String`,
//! `&[u8]` / `Vec<u8>`, `Gc<Table>`, `Gc<LuaClosure>`,
//! `Gc<NativeClosure>`, and `Option<T> where T: IntoValue`.
//!
//! Embedders who want their own host types to flow into `Value`
//! either implement `IntoValue` directly (when they own the type)
//! or write a `From<MyType> for Value` impl and use it explicitly
//! at call sites.

use crate::runtime::heap::Gc;
use crate::runtime::value::Value;
use crate::runtime::{LuaClosure, NativeClosure, Table};
use crate::vm::exec::Vm;

/// Convert `self` into a Lua `Value`, possibly interning through
/// the `Vm`'s heap (for string-shaped types).
pub trait IntoValue {
    /// Convert `self` to a Lua [`Value`], interning strings or allocating
    /// other GC-managed types via `vm.heap` as needed.
    fn into_value(self, vm: &mut Vm) -> Value;
}

// Identity + nil
impl IntoValue for Value {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        self
    }
}

impl IntoValue for () {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Nil
    }
}

// Integers
impl IntoValue for i64 {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Int(self)
    }
}

macro_rules! impl_into_value_int {
    ($($t:ty),+ $(,)?) => {
        $(
            impl IntoValue for $t {
                #[inline]
                fn into_value(self, _vm: &mut Vm) -> Value {
                    Value::Int(self as i64)
                }
            }
        )+
    };
}
impl_into_value_int!(i32, i16, i8, u32, u16, u8);

// Floats
impl IntoValue for f64 {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Float(self)
    }
}
impl IntoValue for f32 {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Float(self as f64)
    }
}

// Bool
impl IntoValue for bool {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Bool(self)
    }
}

// Strings — intern through the heap
impl IntoValue for &str {
    #[inline]
    fn into_value(self, vm: &mut Vm) -> Value {
        Value::Str(vm.heap.intern(self.as_bytes()))
    }
}
impl IntoValue for String {
    #[inline]
    fn into_value(self, vm: &mut Vm) -> Value {
        Value::Str(vm.heap.intern(self.as_bytes()))
    }
}
impl IntoValue for &[u8] {
    #[inline]
    fn into_value(self, vm: &mut Vm) -> Value {
        Value::Str(vm.heap.intern(self))
    }
}
impl IntoValue for Vec<u8> {
    #[inline]
    fn into_value(self, vm: &mut Vm) -> Value {
        Value::Str(vm.heap.intern(&self))
    }
}

// Gc handles — already a Value variant directly
impl IntoValue for Gc<Table> {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Table(self)
    }
}
impl IntoValue for Gc<LuaClosure> {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Closure(self)
    }
}
impl IntoValue for Gc<NativeClosure> {
    #[inline]
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Native(self)
    }
}

// Option<T> — None -> Nil, Some -> inner
impl<T: IntoValue> IntoValue for Option<T> {
    #[inline]
    fn into_value(self, vm: &mut Vm) -> Value {
        match self {
            Some(v) => v.into_value(vm),
            None => Value::Nil,
        }
    }
}
