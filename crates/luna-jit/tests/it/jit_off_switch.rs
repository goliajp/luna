//! Switching the JIT off (`install_null_jit`, the CLI's `--no-jit`) must
//! leave the trace machinery idle: no hot counter ticks and no trace is
//! recorded, so the interpreter pays none of the recorder's costs.

use std::process::Command;

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

// a while loop (Jmp back-edge), a numeric for (ForLoop back-edge) and a
// function called often enough to pass the call threshold
const HOT: &str = r#"
    local function f(x) return x + 1 end
    local s, i = 0, 0
    while i < 500 do i = i + 1; s = s + i end
    for j = 1, 500 do s = f(s) + j end
    return s, f
"#;

#[test]
fn null_jit_leaves_hot_counters_and_recorder_idle() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.install_null_jit();
    assert!(!vm.jit_enabled());
    assert!(!vm.trace_jit_enabled());
    let main = vm.load(HOT.as_bytes(), b"=hot").expect("load");
    let rets = vm.call_value(Value::Closure(main), &[]).expect("run");
    assert!(matches!(rets[0], Value::Int(251_000)), "{:?}", rets[0]);
    let Value::Closure(f) = rets[1] else {
        panic!("expected the function back, got {:?}", rets[1]);
    };
    assert_eq!(main.proto.trace_hot_count.get(), 0);
    assert_eq!(main.proto.call_hot_count.get(), 0);
    assert_eq!(f.proto.call_hot_count.get(), 0);
    assert_eq!(vm.trace_closed_count(), 0);
    assert_eq!(vm.trace_aborted_count(), 0);
    assert_eq!(vm.trace_compiled_count(), 0);
    assert_eq!(vm.trace_compile_failed_count(), 0);
}

#[test]
fn switching_back_on_after_null_jit_records_again() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.install_null_jit();
    luna_jit::install_default_jit(&mut vm);
    vm.set_jit_enabled(true);
    vm.set_trace_jit_enabled(true);
    let main = vm.load(HOT.as_bytes(), b"=hot").expect("load");
    vm.call_value(Value::Closure(main), &[]).expect("run");
    assert!(main.proto.trace_hot_count.get() > 0);
    assert!(vm.trace_closed_count() > 0);
}

#[test]
fn cli_no_jit_records_no_trace() {
    for extra in [&[][..], &["--sandbox"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_luna"))
            .args(["--lua=5.4", "--no-jit", "--profile"])
            .args(extra)
            .args(["-e", HOT])
            .output()
            .expect("spawn luna");
        assert!(out.status.success(), "{out:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        for line in [
            "trace_closed_count: 0",
            "trace_compiled_count: 0",
            "trace_compile_failed_count: 0",
        ] {
            assert!(err.contains(line), "{extra:?}: missing {line:?} in\n{err}");
        }
    }
}
