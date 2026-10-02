// own binary: it sets LUNA_JIT_FIELD_IC, which the jit reads once into a process-wide cache

//! `LUNA_JIT_FIELD_IC=1` turns the table-field inline cache on for every
//! new Vm. The switch itself (on, off, per Vm) is covered in the shared
//! test binary; this checks only that the variable sets its default.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

#[test]
fn env_var_turns_the_field_ic_on_by_default() {
    // SAFETY: the only test in this binary, run before any Vm exists
    unsafe {
        std::env::set_var("LUNA_JIT_FIELD_IC", "1");
    }
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    assert!(vm.field_ic_enabled());
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let r = vm
        .eval(
            "local bucket = { last = 1, rate = 2, tokens = 3 }
             local s = 0
             for i = 1, 1000 do
                 s = s + bucket.last + bucket.rate + bucket.tokens
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], Value::Int(6000)),
        "expected Int(6000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_field_ic_snapshot_count() >= 1,
        "no snapshot captured (compiled={} aborted={})",
        vm.trace_compiled_count(),
        vm.trace_aborted_count(),
    );
}
