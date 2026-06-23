//! P2-C B5 smoke tests: native_typed + FromLuaValue/FromLuaArgs/IntoLuaReturn.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55).open_base().open_math().build()
}

#[test]
fn arity_0_int_return() {
    let mut vm = vm();
    let f = vm.native_typed(|| -> i64 { 42 });
    vm.set_global("answer", f).unwrap();
    let r = vm.eval("return answer()").unwrap();
    assert!(matches!(r[0], Value::Int(42)));
}

#[test]
fn arity_1_int_int() {
    let mut vm = vm();
    let f = vm.native_typed(|x: i64| -> i64 { x * 2 });
    vm.set_global("dbl", f).unwrap();
    let r = vm.eval("return dbl(21)").unwrap();
    assert!(matches!(r[0], Value::Int(42)));
}

#[test]
fn arity_2_add() {
    let mut vm = vm();
    let f = vm.native_typed(|a: i64, b: i64| -> i64 { a + b });
    vm.set_global("add", f).unwrap();
    let r = vm.eval("return add(40, 2)").unwrap();
    assert!(matches!(r[0], Value::Int(42)));
}

#[test]
fn arity_3_string_concat_via_returns_string() {
    let mut vm = vm();
    let f = vm.native_typed(|a: String, b: String, c: String| -> String {
        format!("{a}-{b}-{c}")
    });
    vm.set_global("join3", f).unwrap();
    let r = vm.eval("return join3('a', 'b', 'c')").unwrap();
    assert_eq!(r[0].try_as_str(), Some("a-b-c"));
}

#[test]
fn return_tuple_multi_values() {
    let mut vm = vm();
    let f = vm.native_typed(|x: i64| -> (i64, i64) { (x, x * x) });
    vm.set_global("twice", f).unwrap();
    let r = vm.eval("return twice(7)").unwrap();
    assert_eq!(r.len(), 2);
    assert!(matches!(r[0], Value::Int(7)));
    assert!(matches!(r[1], Value::Int(49)));
}

#[test]
fn return_unit_is_zero_returns() {
    let mut vm = vm();
    let f = vm.native_typed(|_x: i64| -> () { /* no-op */ });
    vm.set_global("noop", f).unwrap();
    let r = vm.eval("return noop(99)").unwrap();
    assert_eq!(r.len(), 0);
}

#[test]
fn return_bool() {
    let mut vm = vm();
    let f = vm.native_typed(|x: i64| -> bool { x > 0 });
    vm.set_global("pos", f).unwrap();
    let r = vm.eval("return pos(5), pos(-1)").unwrap();
    assert_eq!(r.len(), 2);
    assert!(matches!(r[0], Value::Bool(true)));
    assert!(matches!(r[1], Value::Bool(false)));
}

#[test]
fn float_arg_coerced_to_i64_when_integral() {
    let mut vm = vm();
    let f = vm.native_typed(|x: i64| -> i64 { x + 1 });
    vm.set_global("inc", f).unwrap();
    // 5.0 is exactly integral; FromLuaValue::for<i64> coerces it.
    let r = vm.eval("return inc(5.0)").unwrap();
    assert!(matches!(r[0], Value::Int(6)));
}

#[test]
fn result_propagates_error() {
    let mut vm = vm();
    let f = vm.native_typed(|x: i64| -> Result<i64, luna_core::vm::LuaError> {
        if x == 0 {
            Err(luna_core::vm::LuaError(Value::Nil))
        } else {
            Ok(x + 1)
        }
    });
    vm.set_global("guarded_inc", f).unwrap();

    let ok = vm.eval("return guarded_inc(10)").unwrap();
    assert!(matches!(ok[0], Value::Int(11)));

    let err = vm.eval("return guarded_inc(0)").expect_err("should error");
    let _ = err; // just verify it surfaced
}

#[test]
fn option_arg_handles_nil() {
    let mut vm = vm();
    let f = vm.native_typed(|x: Option<i64>| -> i64 { x.unwrap_or(-1) });
    vm.set_global("default", f).unwrap();
    let r = vm.eval("return default(42), default(nil)").unwrap();
    assert!(matches!(r[0], Value::Int(42)));
    assert!(matches!(r[1], Value::Int(-1)));
}

#[test]
fn typed_fn_item_works() {
    fn square(x: i64) -> i64 {
        x * x
    }
    let mut vm = vm();
    let f = vm.native_typed(square as fn(i64) -> i64);
    vm.set_global("sq", f).unwrap();
    let r = vm.eval("return sq(11)").unwrap();
    assert!(matches!(r[0], Value::Int(121)));
}

#[test]
fn arity_4_sum() {
    let mut vm = vm();
    let f = vm.native_typed(|a: i64, b: i64, c: i64, d: i64| -> i64 { a + b + c + d });
    vm.set_global("sum4", f).unwrap();
    let r = vm.eval("return sum4(1, 2, 3, 4)").unwrap();
    assert!(matches!(r[0], Value::Int(10)));
}

#[test]
fn arity_5_sum() {
    let mut vm = vm();
    let f = vm.native_typed(|a: i64, b: i64, c: i64, d: i64, e: i64| -> i64 { a + b + c + d + e });
    vm.set_global("sum5", f).unwrap();
    let r = vm.eval("return sum5(1, 2, 3, 4, 5)").unwrap();
    assert!(matches!(r[0], Value::Int(15)));
}

#[test]
fn arity_6_sum() {
    let mut vm = vm();
    let f = vm.native_typed(
        |a: i64, b: i64, c: i64, d: i64, e: i64, f: i64| -> i64 { a + b + c + d + e + f },
    );
    vm.set_global("sum6", f).unwrap();
    let r = vm.eval("return sum6(1, 2, 3, 4, 5, 6)").unwrap();
    assert!(matches!(r[0], Value::Int(21)));
}
