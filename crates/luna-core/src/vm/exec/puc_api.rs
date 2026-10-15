//! Where PUC's C API checks for a collection inside a library function:
//! 5.1 and 5.2 before the call allocates, 5.3 on after it pushed what it
//! made. The running C function's stack is live up to its top; a value
//! the library function holds is rooted while the collector may run.

use super::*;
use crate::runtime::LuaStr;

impl Vm {
    /// 5.1 and 5.2: the check before an API call allocates.
    #[inline]
    pub(crate) fn api_check_before(&mut self) {
        if self.version <= LuaVersion::Lua52 {
            self.native_checkpoint();
        }
    }

    /// 5.3 on: the check after an API call pushed `v`.
    #[inline]
    pub(crate) fn api_check_after(&mut self, v: Value) {
        if self.version >= LuaVersion::Lua53 {
            self.lib_roots.push(v);
            self.native_checkpoint();
            self.lib_roots.pop();
        }
    }

    /// 5.3 on: the check after an API call pushed a value already rooted
    /// (in `lib_roots`).
    #[inline]
    pub(crate) fn api_check_after_rooted(&mut self) {
        if self.version >= LuaVersion::Lua53 {
            self.native_checkpoint();
        }
    }

    /// `f` as an API call that pushes the string it makes: checked for
    /// before (5.1, 5.2) or after (5.3 on), the string kept rooted in
    /// `lib_roots` for the caller, who truncates it.
    pub(crate) fn api_pushing(
        &mut self,
        f: impl FnOnce(&mut Vm) -> Result<Gc<LuaStr>, LuaError>,
    ) -> Result<Gc<LuaStr>, LuaError> {
        self.api_check_before();
        let r = f(self)?;
        self.lib_roots.push(Value::Str(r));
        self.api_check_after_rooted();
        Ok(r)
    }

    /// The check every version makes after `lua_tolstring` converted a
    /// number into `v`.
    #[inline]
    pub(crate) fn api_check_converted(&mut self, v: Value) {
        self.lib_roots.push(v);
        self.native_checkpoint();
        self.lib_roots.pop();
    }

    /// `lua_pushlstring`: the string of `bytes`, checked for as PUC checks.
    pub(crate) fn api_push_string(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        self.api_check_before();
        let s = self.str_value(bytes)?;
        self.api_check_after(s);
        Ok(s)
    }
}
