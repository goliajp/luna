//! VM semantic corpus: expressions, locals, assignments,
//! control flow, numeric for, tables, globals. Assertions go through chunk
//! return values (no stdlib yet).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn eval(src: &str) -> Vec<Value> {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    }
}

fn eval1(src: &str) -> Value {
    let mut v = eval(src);
    assert_eq!(v.len(), 1, "expected 1 result from {src:?}");
    let v = v.pop().unwrap();
    // GC-backed values would dangle once the Vm drops — assert through a
    // helper that keeps the Vm alive instead (e.g. check_str)
    assert!(
        !matches!(v, Value::Str(_) | Value::Table(_) | Value::Closure(_)),
        "eval1 must not return GC values; use a vm-scoped helper"
    );
    v
}

#[track_caller]
fn check_str(src: &str, expect: &[u8]) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    };
    assert_eq!(v.len(), 1, "expected 1 result from {src:?}");
    match v[0] {
        Value::Str(s) => assert_eq!(
            s.as_bytes(),
            expect,
            "{src:?} → {:?}",
            String::from_utf8_lossy(s.as_bytes())
        ),
        v => panic!("{src:?} → {v:?}, expected a string"),
    }
}

#[track_caller]
fn check_int(src: &str, expect: i64) {
    let v = eval1(src);
    assert!(
        v.raw_eq(Value::Int(expect)) && matches!(v, Value::Int(_)),
        "{src:?} → {v:?}, expected Int({expect})"
    );
}

#[track_caller]
fn check_float(src: &str, expect: f64) {
    let v = eval1(src);
    match v {
        Value::Float(f) => assert!(
            f == expect || (f.is_nan() && expect.is_nan()),
            "{src:?} → {f}, expected {expect}"
        ),
        v => panic!("{src:?} → {v:?}, expected Float({expect})"),
    }
}

#[track_caller]
fn check_bool(src: &str, expect: bool) {
    let v = eval1(src);
    assert!(
        matches!(v, Value::Bool(b) if b == expect),
        "{src:?} → {v:?}, expected Bool({expect})"
    );
}

#[track_caller]
fn check_error(src: &str, contains: &str) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => panic!("{src:?} unexpectedly returned {v:?}"),
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains(contains),
                "{src:?} error {msg:?} does not contain {contains:?}"
            );
        }
    }
}

fn check_compile_error(src: &str, contains: &str) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => panic!("{src:?} unexpectedly compiled and returned {v:?}"),
        Err(e) => {
            let m = vm.error_text(&e);
            assert!(
                m.contains(contains),
                "{src:?} error {m:?} does not contain {contains:?}"
            );
        }
    }
}

mod api;
mod coroutines;
mod coroutines_results;
mod debug;
mod dump;
mod embedding;
mod errors;
mod functions;
mod gc;
mod io;
mod libs;
mod metamethods;
mod scoping;
mod semantics;
mod strings;
mod to_be_closed;
