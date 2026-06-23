//! P3 B8 smoke tests: host userdata payloads + downcast.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaUserdata, Vm};

#[derive(Debug, PartialEq)]
struct Counter(i64);

impl LuaUserdata for Counter {}

#[derive(Debug)]
struct Settings {
    name: String,
    max_clients: u32,
}

impl LuaUserdata for Settings {}

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55).open_base().build()
}

#[test]
fn create_userdata_returns_userdata_value() {
    let mut vm = vm();
    let ud = vm.create_userdata(Counter(42));
    assert!(matches!(ud, Value::Userdata(_)));
}

#[test]
fn set_userdata_and_borrow_back() {
    let mut vm = vm();
    vm.set_userdata("counter", Counter(7)).unwrap();
    let c: &Counter = vm.userdata_borrow("counter").unwrap();
    assert_eq!(c.0, 7);
}

#[test]
fn userdata_borrow_mut_mutates() {
    let mut vm = vm();
    vm.set_userdata("c", Counter(0)).unwrap();
    {
        let c: &mut Counter = vm.userdata_borrow_mut("c").unwrap();
        c.0 = 99;
    }
    let c: &Counter = vm.userdata_borrow("c").unwrap();
    assert_eq!(c.0, 99);
}

#[test]
fn userdata_borrow_wrong_type_returns_none() {
    let mut vm = vm();
    vm.set_userdata("c", Counter(1)).unwrap();
    let s: Option<&Settings> = vm.userdata_borrow("c");
    assert!(s.is_none());
}

#[test]
fn userdata_borrow_missing_global_returns_none() {
    let mut vm = vm();
    let c: Option<&Counter> = vm.userdata_borrow("nope");
    assert!(c.is_none());
}

#[test]
fn userdata_borrow_non_userdata_returns_none() {
    let mut vm = vm();
    vm.set_global("x", 42_i64).unwrap();
    let c: Option<&Counter> = vm.userdata_borrow("x");
    assert!(c.is_none());
}

#[test]
fn create_userdata_with_owned_string() {
    let mut vm = vm();
    let s = Settings {
        name: "redis-prod-01".to_string(),
        max_clients: 1024,
    };
    vm.set_userdata("cfg", s).unwrap();
    let c: &Settings = vm.userdata_borrow("cfg").unwrap();
    assert_eq!(c.name, "redis-prod-01");
    assert_eq!(c.max_clients, 1024);
}

#[test]
fn userdata_survives_gc() {
    let mut vm = vm();
    vm.set_userdata("c", Counter(123)).unwrap();
    vm.collect_garbage();
    let c: &Counter = vm.userdata_borrow("c").unwrap();
    assert_eq!(c.0, 123);
}

#[test]
fn userdata_visible_to_lua_via_type() {
    let mut vm = vm();
    vm.set_userdata("c", Counter(5)).unwrap();
    let r = vm.eval("return type(c)").unwrap();
    assert_eq!(r[0].try_as_str(), Some("userdata"));
}

#[test]
fn downcast_via_value_userdata_match() {
    let mut vm = vm();
    let ud = vm.create_userdata(Counter(11));
    match ud {
        Value::Userdata(g) => {
            // SAFETY: heap is single-threaded; pointer is live.
            let r = unsafe { &*g.as_ptr() };
            assert_eq!(r.downcast::<Counter>().unwrap(), &Counter(11));
            assert!(r.downcast::<Settings>().is_none());
            assert!(r.is_host());
            assert!(!r.is_proxy());
        }
        _ => panic!("expected Userdata"),
    }
}
