//! v1.1 A1 Session A — smoke tests for the JIT trait boundary.
//!
//! Verifies that:
//! 1. `Vm::install_null_jit()` swaps the dispatcher onto the no-op
//!    backend and a trivial Lua program still runs end-to-end.
//! 2. `Vm::install_default_jit()` re-arms the Cranelift backend and a
//!    JIT-eligible body produces the same result the interp would.
//!
//! The bodies are intentionally small; the point isn't perf or trace
//! coverage but to prove the trait routing reaches both backends.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::VmExt as _;

fn run_to_string(version: LuaVersion, src: &str, install_null: bool) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    if install_null {
        vm.install_null_jit();
    } else {
        vm.install_default_jit();
    }
    let cl = vm.load(src.as_bytes(), b"=chunk").expect("load ok");
    let rets = vm
        .call_value(Value::Closure(cl), &[])
        .expect("call ok");
    rets.into_iter()
        .map(|v| match v {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Value::Nil => "nil".into(),
            other => format!("{:?}", other),
        })
        .collect::<Vec<_>>()
        .join("\t")
}

#[test]
fn null_jit_runs_trivial_print() {
    // `print` writes to stdout; we just confirm the program returns
    // without panicking and the interp path produced a clean value
    // return. Using `return` is enough — the chunk evaluates to an
    // integer that the dispatcher routes through the interp arms.
    let out = run_to_string(LuaVersion::Lua55, "return 1 + 2", true);
    assert_eq!(out, "3");
}

#[test]
fn null_jit_runs_arithmetic_loop_correctly() {
    // A small counted-for loop the cranelift backend would normally
    // pick up; under NullJitBackend the dispatcher takes the
    // interpreter path. Both must produce identical results.
    let src = r#"
        local s = 0
        for i = 1, 100 do
            s = s + i
        end
        return s
    "#;
    let out_null = run_to_string(LuaVersion::Lua55, src, true);
    let out_jit = run_to_string(LuaVersion::Lua55, src, false);
    assert_eq!(out_null, "5050");
    assert_eq!(out_jit, "5050");
    assert_eq!(out_null, out_jit);
}

#[test]
fn install_default_jit_after_null_restores_routing() {
    // Confirm the trait fields are swappable in either direction:
    // start with null, then swap to default; the dispatcher must
    // accept either configuration without changing observable output.
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.install_null_jit();
    vm.install_default_jit();
    let cl = vm
        .load(b"return 7 * 6", b"=chunk")
        .expect("load ok");
    let rets = vm
        .call_value(Value::Closure(cl), &[])
        .expect("call ok");
    assert_eq!(rets.len(), 1);
    match rets[0] {
        Value::Int(42) => {}
        other => panic!("expected Int(42), got {:?}", other),
    }
}

#[test]
fn null_jit_handles_function_call_path() {
    // Exercises `populate_jit_cache` → `chunk_compiler.try_compile`
    // → CompileResult::Skipped under the null backend. Without the
    // routing this would explode (it used to call into Cranelift
    // unconditionally).
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.install_null_jit();
    let cl = vm
        .load(
            br#"
                local function f(n)
                    if n < 2 then return n end
                    return f(n - 1) + f(n - 2)
                end
                return f(10)
            "#,
            b"=chunk",
        )
        .expect("load ok");
    let rets = vm
        .call_value(Value::Closure(cl), &[])
        .expect("call ok");
    assert_eq!(rets.len(), 1);
    match rets[0] {
        Value::Int(55) => {}
        other => panic!("expected fib(10)=55, got {:?}", other),
    }
}
