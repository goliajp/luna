//! Opening one standard library at a time, for the C API's `luaopen_*`.
//!
//! From 5.2 on a `luaopen_*` function only makes its library and returns
//! it: the global and `package.loaded` entries are `luaL_requiref`'s job.
//! 5.1's register the library themselves (`luaL_register`): into the
//! table `package.loaded` or the global of that name already holds, if
//! any, else into a new one stored in both.

use super::*;

/// Why `Vm::host_open_lib` failed.
pub enum HostOpenError {
    /// 5.1's "name conflict for module": the global of this name holds a
    /// value that is not a table
    Conflict(String),
    /// an error raised while opening (a memory error)
    Error(LuaError),
}

impl From<LuaError> for HostOpenError {
    fn from(e: LuaError) -> HostOpenError {
        HostOpenError::Error(e)
    }
}

impl Vm {
    /// Open the standard library `name` (`_G` for the base library) as the
    /// dialect's `luaopen_*` does, and return what that function returns.
    pub(crate) fn host_open_lib(&mut self, name: &str) -> Result<Vec<Value>, HostOpenError> {
        Ok(self.run_open_body(name)?)
    }
}
