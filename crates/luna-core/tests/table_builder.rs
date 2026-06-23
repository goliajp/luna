//! P2-B B3+B4 smoke tests: IntoValue + TableBuilder + table_of + generic set_global.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn sandbox(version: LuaVersion) -> Vm {
    Vm::sandbox(version).open_base().open_math().build()
}

#[test]
fn set_global_i64() {
    let mut vm = sandbox(LuaVersion::Lua55);
    vm.set_global("answer", 42_i64).unwrap();
    let r = vm.eval("return answer").unwrap();
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Value::Int(42)));
}

#[test]
fn set_global_f64_str_bool_nil() {
    let mut vm = sandbox(LuaVersion::Lua55);
    vm.set_global("pi", 3.14_f64).unwrap();
    vm.set_global("name", "luna").unwrap();
    vm.set_global("flag", true).unwrap();
    vm.set_global("nothing", ()).unwrap();
    let r = vm
        .eval("return type(pi), type(name), type(flag), type(nothing)")
        .unwrap();
    assert_eq!(r.len(), 4);
    assert_eq!(r[0].try_as_str(), Some("number"));
    assert_eq!(r[1].try_as_str(), Some("string"));
    assert_eq!(r[2].try_as_str(), Some("boolean"));
    assert_eq!(r[3].try_as_str(), Some("nil"));
}

#[test]
fn set_global_value_identity_still_works() {
    let mut vm = sandbox(LuaVersion::Lua55);
    // Pre-B4 callers passing `Value::*` directly still compile.
    vm.set_global("seven", Value::Int(7)).unwrap();
    let r = vm.eval("return seven").unwrap();
    assert!(matches!(r[0], Value::Int(7)));
}

#[test]
fn set_global_option_none_is_nil() {
    let mut vm = sandbox(LuaVersion::Lua55);
    let none: Option<i64> = None;
    let some: Option<i64> = Some(5);
    vm.set_global("nothing", none).unwrap();
    vm.set_global("something", some).unwrap();
    let r = vm
        .eval("return type(nothing), type(something), something")
        .unwrap();
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].try_as_str(), Some("nil"));
    assert_eq!(r[1].try_as_str(), Some("number"));
    assert!(matches!(r[2], Value::Int(5)));
}

#[test]
fn table_of_basic() {
    let mut vm = sandbox(LuaVersion::Lua55);
    let t = vm.table_of([("answer", 42_i64), ("year", 2026)]);
    vm.set_global("c", Value::Table(t)).unwrap();
    let r = vm.eval("return c.answer + c.year").unwrap();
    assert!(matches!(r[0], Value::Int(2068)));
}

#[test]
fn new_table_builder_chain() {
    let mut vm = sandbox(LuaVersion::Lua55);
    let t = vm
        .new_table()
        .with("name", "luna")
        .with("major", 1_i64)
        .with("minor", 1_i64)
        .build();
    vm.set_global("info", Value::Table(t)).unwrap();
    let r = vm
        .eval("return info.name, info.major + info.minor")
        .unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].try_as_str(), Some("luna"));
    assert!(matches!(r[1], Value::Int(2)));
}

#[test]
fn new_table_mixed_value_kinds() {
    let mut vm = sandbox(LuaVersion::Lua55);
    let t = vm
        .new_table()
        .with("int_key", 100_i64)
        .with("float_key", 3.5_f64)
        .with("bool_key", false)
        .with(1_i64, "first")
        .with(2_i64, "second")
        .build();
    vm.set_global("t", Value::Table(t)).unwrap();
    let r = vm
        .eval("return t.int_key, t.float_key, t.bool_key, t[1], t[2]")
        .unwrap();
    assert_eq!(r.len(), 5);
    assert!(matches!(r[0], Value::Int(100)));
    if let Value::Float(f) = r[1] {
        assert!((f - 3.5).abs() < 1e-9);
    } else {
        panic!("expected float, got {:?}", r[1]);
    }
    assert!(matches!(r[2], Value::Bool(false)));
    assert_eq!(r[3].try_as_str(), Some("first"));
    assert_eq!(r[4].try_as_str(), Some("second"));
}

#[test]
fn try_with_propagates_overflow() {
    // We can't actually trigger Overflow (MAX_ASIZE = 1<<27); this
    // test just verifies the signature compiles and a normal
    // try_with succeeds.
    let mut vm = sandbox(LuaVersion::Lua55);
    let t = vm.new_table().try_with("k", "v").unwrap().build();
    vm.set_global("t", Value::Table(t)).unwrap();
    let r = vm.eval("return t.k").unwrap();
    assert_eq!(r[0].try_as_str(), Some("v"));
}
