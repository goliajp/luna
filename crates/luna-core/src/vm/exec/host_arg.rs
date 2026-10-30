//! lua.c's `arg` table and its stack checks, made through the API calls
//! lua.c makes.

use super::*;

impl Vm {
    /// lua.c's `arg` table (`createargtable`; 5.1 and 5.2 `getargs`, which
    /// first push the script's arguments), made through the API calls
    /// lua.c makes, with their checks for a collection: `arg[0]` is
    /// `argv[script]`, the script's arguments count up from 1 and what
    /// comes before it down from -1.
    pub(crate) fn host_set_arg(&mut self, argv: &[Vec<u8>], script: usize) -> Result<(), LuaError> {
        let base = self.lib_roots.mark();
        let r = with_rules!(self, P => self.host_arg_table::<P>(argv, script));
        self.lib_roots.release(base);
        let t = r?;
        self.set_global("arg", Value::Table(t))
    }

    /// `luaL_checkstack(L, n, msg)` in lua.c's `pmain` with `values` values
    /// on its stack (above its function at slot 0), reserved from then on.
    pub(crate) fn host_check_stack(
        &mut self,
        values: u32,
        n: u32,
        msg: &str,
    ) -> Result<(), LuaError> {
        crate::vm::argcheck::check_stack_at(self, 1 + values, i64::from(n), msg)
    }

    fn host_arg_table<const PRE: bool>(
        &mut self,
        argv: &[Vec<u8>],
        script: usize,
    ) -> Result<Gc<Table>, LuaError> {
        let narg = argv.len() - script - 1;
        if self.version <= LuaVersion::Lua52 {
            // `getargs`, with `pmain`'s arguments on the stack
            let msg = "too many arguments to script";
            self.host_check_stack(self.g.host_slots - 1, narg as u32 + 3, msg)?;
            for a in &argv[script + 1..] {
                let s = self.api_push_string::<PRE>(a)?;
                self.lib_roots.push(s);
            }
        }
        self.api_check_before::<PRE>()?;
        let t = match self.table_op(|vm| vm.heap.try_create_table(narg, script + 1)) {
            Ok(t) => t,
            Err(e) => return Err(self.table_error(e)),
        };
        self.lib_roots.push(Value::Table(t));
        self.api_check_after_rooted::<PRE>()?;
        for (i, a) in argv.iter().enumerate() {
            let s = self.api_push_string::<PRE>(a)?;
            let k = Value::Int(i as i64 - script as i64);
            self.raw_set(t, k, s)?;
        }
        Ok(t)
    }
}
