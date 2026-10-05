//! A recording that failed to compile in one Vm of an engine is not
//! compiled again by another: the second Vm counts the first one's
//! failures as its own and gives the head up after one recording. A
//! recording with other entry types, or that took another path, compiles
//! as usual.

use super::*;

/// The loop writes an upvalue, which the trace lowering declines; with
/// `FLAG` false it writes only a local, and compiles.
const FAILS: &str = r#"
local c = 0
return function(x)
    local s = 0
    for i = 1, 300 do
        if FLAG then c = c + x else s = s + x end
    end
    return c, s
end
"#;

/// What running `FAILS` with argument `x` and `FLAG` = `flag` did in `vm`.
fn run_fails(vm: &mut Vm, x: Value, flag: bool, calls: usize) -> (Vec<String>, u64, u64, u64) {
    let closed = vm.trace_closed_count();
    let failed = vm.trace_compile_failed_count();
    let compiled = vm.trace_compiled_count();
    vm.set_global("FLAG", Value::Bool(flag)).expect("global");
    let main = vm.load(FAILS.as_bytes(), b"=t").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    let results = (0..calls)
        .map(|_| match vm.call_value(Value::Closure(f), &[x]) {
            Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect();
    (
        results,
        vm.trace_closed_count() - closed,
        vm.trace_compile_failed_count() - failed,
        vm.trace_compiled_count() - compiled,
    )
}

#[test]
fn second_vm_does_not_compile_a_failing_recording_again() {
    let want = run_fails(&mut interp(LuaVersion::Lua54), Value::Int(2), true, 4).0;
    let engine = Engine::new();
    let mut a = shared(&engine, LuaVersion::Lua54);
    let (got, a_closed, a_failed, _) = run_fails(&mut a, Value::Int(2), true, 4);
    assert_eq!(got, want);
    assert!(a_failed >= 2, "the first Vm failed {a_failed} times");
    let mut b = shared(&engine, LuaVersion::Lua54);
    let (got, b_closed, b_failed, b_compiled) = run_fails(&mut b, Value::Int(2), true, 4);
    assert_eq!(got, want);
    assert_eq!(b_failed, 0, "the second Vm compiled and failed again");
    assert_eq!(b_compiled, 0);
    assert_eq!(
        b_closed, 1,
        "the second Vm recorded more than once (first: {a_closed})"
    );
    assert_eq!(b.trace_shared_failures_known(), 1);
    assert_eq!(b.trace_shared_failures_counted(), a_failed);
}

#[test]
fn other_entry_types_record_and_compile_as_usual() {
    let engine = Engine::new();
    let mut a = shared(&engine, LuaVersion::Lua54);
    let (_, _, a_failed, _) = run_fails(&mut a, Value::Int(2), true, 4);
    assert!(a_failed > 0);
    // a float in the register the loop reads: another entry type
    let want = run_fails(&mut interp(LuaVersion::Lua54), Value::Float(0.5), true, 4).0;
    let mut b = shared(&engine, LuaVersion::Lua54);
    let (got, _, b_failed, _) = run_fails(&mut b, Value::Float(0.5), true, 4);
    assert_eq!(got, want);
    assert_eq!(b.trace_shared_failures_known(), 0);
    assert!(b_failed > 0, "the second Vm did not try its own recording");
}

#[test]
fn another_path_compiles() {
    let engine = Engine::new();
    let mut a = shared(&engine, LuaVersion::Lua54);
    let (_, _, a_failed, _) = run_fails(&mut a, Value::Int(2), true, 4);
    assert!(a_failed > 0);
    let want = run_fails(&mut interp(LuaVersion::Lua54), Value::Int(2), false, 4).0;
    let mut b = shared(&engine, LuaVersion::Lua54);
    let (got, _, b_failed, b_compiled) = run_fails(&mut b, Value::Int(2), false, 4);
    assert_eq!(got, want);
    assert_eq!(b.trace_shared_failures_known(), 0);
    assert_eq!(b_failed, 0);
    assert!(b_compiled > 0, "the second Vm's own path did not compile");
    assert!(b.trace_dispatched_count() > 0);
}

#[test]
fn a_vm_without_an_engine_fails_on_its_own() {
    let mut a = luna_jit::new_with_jit(LuaVersion::Lua54);
    a.set_jit_enabled(false);
    a.jit.trace_hot_threshold = 8;
    let (_, _, a_failed, _) = run_fails(&mut a, Value::Int(2), true, 4);
    let mut b = luna_jit::new_with_jit(LuaVersion::Lua54);
    b.set_jit_enabled(false);
    b.jit.trace_hot_threshold = 8;
    let (_, _, b_failed, _) = run_fails(&mut b, Value::Int(2), true, 4);
    assert_eq!(a_failed, b_failed);
    assert_eq!(b.trace_shared_failures_known(), 0);
}
