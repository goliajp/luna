//! A memory error runs no message handler: PUC throws `LUA_ERRMEM` past
//! the handler of the `xpcall` that catches it, in every dialect.

use luna_core::runtime::Value;
use luna_core::runtime::mem::{MemOwner, MemoryLimit};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[test]
fn xpcall_runs_no_handler_on_a_memory_error() {
    let source = "local ok, m = xpcall(function()
           local t = {}
           for i = 1, 1e7 do t[i] = i end
         end, function(m) return 'handled: ' .. tostring(m) end)
         collectgarbage()
         return tostring(ok) .. ' ' .. tostring(m)";
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mem = MemOwner::policy(Box::new(MemoryLimit(8 << 20)));
        let mut vm = Vm::new_with_mem(v, mem);
        let f = vm.load(source.as_bytes(), b"=chunk").expect("loads");
        let r = vm.call_value(Value::Closure(f), &[]).expect("runs");
        let got = match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        };
        assert_eq!(got, "false not enough memory", "{v:?}");
    }
}
