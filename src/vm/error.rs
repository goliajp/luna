//! Runtime errors: a Lua error is an arbitrary Lua value (usually a
//! string). Propagated as `Result<_, LuaError>` through the interpreter;
//! `pcall` catches it at the native boundary. Traceback capture lands with
//! the debug interfaces (P05).

use crate::runtime::Value;

#[derive(Clone, Copy, Debug)]
pub struct LuaError(pub Value);
