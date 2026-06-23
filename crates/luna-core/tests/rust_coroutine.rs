//! B9 smoke tests: Rust-side coroutine drive.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55)
        .open_base()
        .open_coroutine()
        .build()
}

#[test]
fn create_coroutine_returns_coro_value() {
    let mut vm = vm();
    let body = vm.native_typed(|| -> i64 { 42 });
    let co = vm.create_coroutine(body);
    assert!(matches!(co, Value::Coro(_)));
}

#[test]
fn resume_coroutine_runs_body() {
    let mut vm = vm();
    let body = vm.native_typed(|| -> i64 { 7 });
    let co = vm.create_coroutine(body);
    let r = vm.resume_coroutine(co, vec![]).unwrap();
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Value::Int(7)));
}

#[test]
fn resume_coroutine_with_args_via_lua_body() {
    let mut vm = vm();
    // Compile a Lua function: function(a, b) return a + b end
    let r = vm
        .eval("return function(a, b) return a + b end")
        .unwrap();
    let body = r.into_iter().next().unwrap();
    let co = vm.create_coroutine(body);
    let result = vm
        .resume_coroutine(co, vec![Value::Int(10), Value::Int(32)])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], Value::Int(42)));
}

#[test]
fn resume_non_coroutine_returns_error() {
    let mut vm = vm();
    let err = vm
        .resume_coroutine(Value::Int(42), vec![])
        .expect_err("non-Coro must error");
    let _ = err;
}

#[test]
fn coroutine_yield_and_resume_returns_yielded_values() {
    let mut vm = vm();
    let r = vm
        .eval(
            r#"return function()
                coroutine.yield(1)
                coroutine.yield(2)
                return 3
            end"#,
        )
        .unwrap();
    let body = r.into_iter().next().unwrap();
    let co = vm.create_coroutine(body);

    // 1st resume — yields 1
    let r1 = vm.resume_coroutine(co, vec![]).unwrap();
    assert!(matches!(r1[0], Value::Int(1)), "got {:?}", r1);

    // 2nd resume — yields 2
    let r2 = vm.resume_coroutine(co, vec![]).unwrap();
    assert!(matches!(r2[0], Value::Int(2)), "got {:?}", r2);

    // 3rd resume — terminal return 3
    let r3 = vm.resume_coroutine(co, vec![]).unwrap();
    assert!(matches!(r3[0], Value::Int(3)), "got {:?}", r3);
}
