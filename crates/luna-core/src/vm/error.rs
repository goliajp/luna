//! Runtime errors: a Lua error is an arbitrary Lua value (usually a
//! string). Propagated as `Result<_, LuaError>` through the interpreter;
//! `pcall` catches it at the native boundary. Traceback capture lands with
//! the debug interfaces (P05).

use crate::runtime::Value;
use crate::runtime::table::TableError;

#[derive(Clone, Copy, Debug)]
pub struct LuaError(pub Value);

impl From<TableError> for LuaError {
    /// Lift a TableError into a LuaError carrying a `Value::Nil` placeholder.
    /// Callers that want a rich message use `LuaError::table_error_with_vm`
    /// (when implemented in B6) to attach an interned `Value::Str`. For now
    /// the conversion is heap-free so it composes with `?` in functions
    /// that don't carry a `&mut Vm`.
    fn from(_: TableError) -> LuaError {
        LuaError(Value::Nil)
    }
}
