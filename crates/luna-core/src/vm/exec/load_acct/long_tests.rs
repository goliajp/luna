//! A long name scanned again is another string of the same bytes, which
//! the scanner's table already holds one of: the load goes on with that
//! one, as PUC's `luaX_newstring` does, and the new one is garbage. A
//! collection stepped while the load runs must not free the one it uses.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::Vm;

#[test]
fn a_long_name_scanned_again_uses_the_string_the_table_holds() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = Vm::new(v);
        let r = vm
            .eval(
                "for _, p in ipairs({{'setpause', 0}, {'setstepmul', 1000}, {'param', 'pause', 0},
                     {'param', 'stepmul', 1000}}) do pcall(collectgarbage, table.unpack(p)) end
                 local a = string.rep('a', 15000)
                 local n1, n2 = a .. '1', a .. '2'
                 local prog = string.format('%s = 5 %s = %s + 1 return function () return %s - %s end',
                   n1, n2, n1, n1, n2)
                 local f = load(prog)()
                 return f()",
            )
            .expect("the script runs");
        assert!(
            matches!(r[0], Value::Int(-1) | Value::Float(-1.0)),
            "{v:?}: {:?}",
            r[0]
        );
    }
}
