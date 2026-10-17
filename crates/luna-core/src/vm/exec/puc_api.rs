//! Where PUC's C API checks for a collection inside a library function:
//! 5.1 and 5.2 before the call allocates, 5.3 on after it pushed what it
//! made. The running C function's stack is live up to its top; a value
//! the library function holds is rooted while the collector may run.

use super::*;
use crate::runtime::LuaStr;

impl Vm {
    /// 5.1 and 5.2: the check before an API call allocates.
    #[inline]
    pub(crate) fn api_check_before(&mut self) -> Result<(), LuaError> {
        if self.version <= LuaVersion::Lua52 {
            return self.native_checkpoint();
        }
        Ok(())
    }

    /// 5.3 on: the check after an API call pushed `v`.
    #[inline]
    pub(crate) fn api_check_after(&mut self, v: Value) -> Result<(), LuaError> {
        if self.version >= LuaVersion::Lua53 && !self.checkpoint_idle() {
            self.lib_roots.push(v);
            let r = self.native_checkpoint_due();
            self.lib_roots.pop();
            return r;
        }
        Ok(())
    }

    /// 5.3 on: the check after an API call pushed a value already rooted
    /// (in `lib_roots`).
    #[inline]
    pub(crate) fn api_check_after_rooted(&mut self) -> Result<(), LuaError> {
        if self.version >= LuaVersion::Lua53 {
            return self.native_checkpoint();
        }
        Ok(())
    }

    /// PUC `luaL_getmetafield(L, idx, event)` on `v`: when `v` has a
    /// metatable it pushes the event's name (`lua_pushstring`, which makes
    /// the string and checks for a collection), the string given back.
    #[inline]
    pub(crate) fn metafield_name(
        &mut self,
        v: Value,
        event: &[u8],
    ) -> Result<Option<Gc<LuaStr>>, LuaError> {
        if self.metatable_of(v).is_none() {
            return Ok(None);
        }
        self.api_check_before()?;
        // by the name's address, as PUC's string cache is; emptied when a
        // collection step begins, so a string found here is not dead
        let key = event.as_ptr() as usize;
        let slot = (key >> 3) % self.name_cache.len();
        let s = match self.name_cache[slot] {
            (k, Some(s)) if k == key => s,
            _ => {
                let s = self.alloc_str(event)?;
                self.name_cache[slot] = (key, Some(s));
                s
            }
        };
        self.api_check_after(Value::Str(s))?;
        Ok(Some(s))
    }

    /// [`Vm::metafield_name`] where only the push matters.
    #[inline]
    pub(crate) fn metafield_check(&mut self, v: Value, event: &[u8]) -> Result<(), LuaError> {
        self.metafield_name(v, event).map(drop)
    }

    /// `f` as an API call that pushes the string it makes: checked for
    /// before (5.1, 5.2) or after (5.3 on), the string kept rooted in
    /// `lib_roots` for the caller, who truncates it.
    pub(crate) fn api_pushing(
        &mut self,
        f: impl FnOnce(&mut Vm) -> Result<Gc<LuaStr>, LuaError>,
    ) -> Result<Gc<LuaStr>, LuaError> {
        self.api_check_before()?;
        let r = f(self)?;
        self.lib_roots.push(Value::Str(r));
        self.api_check_after_rooted()?;
        Ok(r)
    }

    /// The check every version makes after `lua_tolstring` converted a
    /// number into `v`.
    #[inline]
    pub(crate) fn api_check_converted(&mut self, v: Value) -> Result<(), LuaError> {
        self.lib_roots.push(v);
        let r = self.native_checkpoint();
        self.lib_roots.pop();
        r
    }

    /// `lua_pushlstring`: the string of `bytes`, checked for as PUC checks.
    pub(crate) fn api_push_string(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        self.api_check_before()?;
        let s = self.str_value(bytes)?;
        self.api_check_after(s)?;
        Ok(s)
    }

    /// lua.c's `arg` table (`createargtable`; 5.1 and 5.2 `getargs`, which
    /// first push the script's arguments), made through the API calls
    /// lua.c makes, with their checks for a collection: `arg[0]` is
    /// `argv[script]`, the script's arguments count up from 1 and what
    /// comes before it down from -1.
    #[doc(hidden)]
    pub fn host_set_arg(&mut self, argv: &[Vec<u8>], script: usize) -> Result<(), LuaError> {
        let base = self.lib_roots.len();
        let r = self.host_arg_table(argv, script);
        self.lib_roots.truncate(base);
        let t = r?;
        self.set_global("arg", Value::Table(t))
    }

    fn host_arg_table(&mut self, argv: &[Vec<u8>], script: usize) -> Result<Gc<Table>, LuaError> {
        if self.version <= LuaVersion::Lua52 {
            for a in &argv[script + 1..] {
                let s = self.api_push_string(a)?;
                self.lib_roots.push(s);
            }
        }
        self.api_check_before()?;
        let narg = argv.len() - script - 1;
        let t = match self.table_op(|vm| vm.heap.try_create_table(narg, script + 1)) {
            Ok(t) => t,
            Err(e) => return Err(self.table_error(e)),
        };
        self.lib_roots.push(Value::Table(t));
        self.api_check_after_rooted()?;
        for (i, a) in argv.iter().enumerate() {
            let s = self.api_push_string(a)?;
            let k = Value::Int(i as i64 - script as i64);
            self.raw_set(t, k, s)?;
        }
        Ok(t)
    }
}
