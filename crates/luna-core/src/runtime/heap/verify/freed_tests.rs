//! The freed addresses gc-verify remembers are forgotten when a new object
//! takes one: a lookup with the new object as its key is not a use of the
//! freed one.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::Vm;

const MAKE: &str = "t = {}
for i = 1, 2000 do t[i] = string.rep('r', 50) .. string.format('%04d', i) end
return t";

#[test]
fn an_address_a_new_object_takes_is_no_longer_freed() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let r = vm
        .eval("s = string.rep('q', 50) .. 'xxxx' return s")
        .expect("the script runs");
    let Value::Str(s) = r[0] else {
        panic!("s is a string");
    };
    let old = s.as_ptr() as usize;
    vm.eval("s = nil collectgarbage()")
        .expect("the script runs");
    assert!(crate::runtime::gc_verify_probe::is_freed(old));
    // strings of the same size until the allocator gives the address back
    let r = vm.eval(MAKE).expect("the script runs");
    let Value::Table(t) = r[0] else {
        panic!("t is a table");
    };
    let reused = (1..=2000)
        .any(|i| matches!(t.get(Value::Int(i)), Value::Str(n) if n.as_ptr() as usize == old));
    assert!(reused, "no new string took the freed address");
    assert!(!crate::runtime::gc_verify_probe::is_freed(old));
    // a lookup keyed by the new string checks its key
    vm.eval("local u = {} for _, v in ipairs(t) do u[v] = true end")
        .expect("the script runs");
}
