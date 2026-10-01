//! Refilling a hole low in a long array part scans at most 64 slots ahead
//! for the new end of the non-nil prefix, so the prefix it records can lag
//! behind the real run. Growing that array part must accept a lagging
//! prefix (it only turns off the `#t` shortcut) and keep lengths right.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn eval_int(src: &str) -> i64 {
    let mut vm = Vm::new(LuaVersion::Lua54);
    match vm.eval(src).expect("eval").first() {
        Some(Value::Int(n)) => *n,
        other => panic!("expected an integer, got {other:?}"),
    }
}

#[test]
fn a_full_array_part_with_a_lagging_prefix_grows() {
    let src = "local t = {}
               for i = 1, 256 do t[i] = i end
               t[1] = nil t[1] = 1
               t[257] = 257
               return #t";
    assert_eq!(eval_int(src), 257);
}

#[test]
fn a_partly_filled_array_part_with_a_lagging_prefix_grows() {
    let src = "local t = {}
               for i = 1, 256 do t[i] = i end
               t[1] = nil t[1] = 1
               t[200] = nil
               for i = 257, 600 do t[i] = i end
               t[200] = 200
               return #t";
    assert_eq!(eval_int(src), 600);
}
