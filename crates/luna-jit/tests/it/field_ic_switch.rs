//! The table-field inline cache, switched per Vm. On, the recorder
//! captures a snapshot at the first eligible `GetField` and the lowering
//! reads the field through the cache; off, neither happens. Either way
//! the program's result is the same.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

// three constant-key reads of a metatable-less table in a hot loop
const SRC: &str = "local bucket = { last = 1, rate = 2, tokens = 3 }
     local s = 0
     for i = 1, 1000 do
         s = s + bucket.last + bucket.rate + bucket.tokens
     end
     return s";

fn run(field_ic: bool) -> luna_jit::Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_field_ic_enabled(field_ic);
    assert_eq!(vm.field_ic_enabled(), field_ic);
    let r = vm.eval(SRC).unwrap();
    assert!(
        matches!(r[0], Value::Int(6000)),
        "field IC {field_ic}: expected Int(6000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "field IC {field_ic}: no trace compiled (compile_failed={} closed={} aborted={})",
        vm.trace_compile_failed_count(),
        vm.trace_closed_count(),
        vm.trace_aborted_count(),
    );
    vm
}

#[test]
fn field_ic_fires_when_switched_on() {
    let vm = run(true);
    assert!(vm.trace_field_ic_snapshot_count() >= 1);
}

#[test]
fn field_ic_does_not_fire_when_switched_off() {
    let vm = run(false);
    assert_eq!(vm.trace_field_ic_snapshot_count(), 0);
}

// the switch belongs to the Vm: one Vm turning it on leaves another off
#[test]
fn field_ic_switch_is_per_vm() {
    let on = run(true);
    let off = run(false);
    assert!(on.trace_field_ic_snapshot_count() >= 1);
    assert_eq!(off.trace_field_ic_snapshot_count(), 0);
}
