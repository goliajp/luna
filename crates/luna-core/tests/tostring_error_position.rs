//! `'__tostring' must return a string` is raised with luaL_error, so it is
//! positioned at whatever called the library function (luaL_where(1)). 5.3's
//! `print` reaches `tostring` through lua_call — a C caller, no position —
//! while 5.4+ `print` calls luaL_tolstring itself, so its Lua caller's
//! position shows. The diff_puc harness cannot pin this: it replaces `print`
//! with a Lua function on luna's side.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn print_error(version: LuaVersion) -> String {
    let mut vm = Vm::new(version);
    let r = vm
        .eval(
            "local bad = setmetatable({}, {__tostring = function() return nil end})\n\
             local _, e = pcall(function() print(bad) end)\n\
             return e",
        )
        .expect("chunk runs");
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected an error string, got {other:?}"),
    }
}

#[test]
fn print_on_5_3_raises_without_a_position() {
    assert_eq!(
        print_error(LuaVersion::Lua53),
        "'__tostring' must return a string"
    );
}

#[test]
fn print_on_5_4_raises_at_its_lua_caller() {
    let e = print_error(LuaVersion::Lua54);
    assert!(
        e.ends_with(":2: '__tostring' must return a string"),
        "unexpected message: {e}"
    );
}
