//! What PUC's `hookf`, the debug library's trampoline between the engine
//! and a Lua hook, leaves on the stack below the hook it calls.

use super::*;

/// The registry key of the debug library's hook table: a light userdata
/// in 5.1 and 5.3, in the others a string, whose bytes are the `Err`.
pub(crate) fn hook_table_key(v: LuaVersion) -> Result<Value, &'static [u8]> {
    match v {
        LuaVersion::Lua51 | LuaVersion::Lua53 => Ok(light_key(HOOK_KEY)),
        LuaVersion::Lua52 => Err(b"_HKEY"),
        _ => Err(b"_HOOKKEY"),
    }
}

impl Vm {
    /// `hookf` pushes the hook table at `top`, below the hook.
    pub(super) fn hookf_push(&mut self, top: u32) -> Result<(), LuaError> {
        let table = self.hookf_table();
        self.grow_stack(top as usize + 1)?;
        self.stack[top as usize] = table;
        Ok(())
    }

    /// The hook table `hookf` reads from the registry, found without
    /// making anything (nil when the key or the table is not there).
    fn hookf_table(&self) -> Value {
        let Some(reg) = self.registry else {
            return Value::Nil;
        };
        let key = match hook_table_key(self.version) {
            Ok(k) => k,
            Err(name) => match self.heap.find_interned(name) {
                Some(s) => Value::Str(s),
                None => return Value::Nil,
            },
        };
        reg.get(key)
    }
}
