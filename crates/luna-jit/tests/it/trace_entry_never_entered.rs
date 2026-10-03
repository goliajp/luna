//! A root trace that reads, on entry, a register holding a value no trace
//! is entered with (a boolean here) could never run: it is not compiled
//! and does not keep its head from a later recording that can run.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn vm() -> Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    // the method JIT would run these functions instead of the interpreter
    // the trace recorder watches
    vm.set_jit_enabled(false);
    vm.jit.call_hot_threshold = 2;
    vm.jit.trace_hot_threshold = 2;
    vm
}

fn function(vm: &mut Vm, src: &str) -> Value {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let r = vm.call_value(Value::Closure(main), &[]).expect("chunk");
    assert!(
        matches!(r[0], Value::Closure(_)),
        "chunk returned no function"
    );
    r[0]
}

fn int(vm: &mut Vm, f: Value, args: &[Value]) -> i64 {
    match vm.call_value(f, args).expect("call")[..] {
        [Value::Int(n), ..] => n,
        ref other => panic!("not an integer: {other:?}"),
    }
}

/// A call-triggered trace: its first op reads the flag.
const CALLED: &str = r#"
return function(flag, a)
    local s = 0
    if flag then s = 1 end
    s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2  s = s - a
    s = s + 11 s = s * 3  s = s - a  s = s + 5  s = s * 2  s = s - 1
    return s
end
"#;

#[test]
fn a_call_trace_entered_with_a_boolean_is_not_compiled_and_leaves_its_head_free() {
    let mut vm = vm();
    let f = function(&mut vm, CALLED);
    for k in 0..20 {
        int(&mut vm, f, &[Value::Bool(false), Value::Int(k)]);
    }
    assert_eq!(
        vm.trace_compiled_count(),
        0,
        "a trace that reads a boolean on entry was compiled"
    );
    let failed = vm.jit.counters.compile_failed;
    assert!(failed > 0, "no recording reached the compiler");
    for k in 0..20 {
        int(&mut vm, f, &[Value::Int(5), Value::Int(k)]);
    }
    assert!(
        vm.trace_compiled_count() > 0,
        "the head stayed given up after the boolean calls"
    );
}

/// A loop whose body tests a flag that stays live around the loop.
const LOOPED: &str = r#"
return function(flag, n)
    local s = 0
    for i = 1, n do
        if flag then s = s + i else s = s - 1 end
        s = s * 3 % 1000003
    end
    return s
end
"#;

fn looped_by_hand(flag: bool, n: i64) -> i64 {
    let mut s: i64 = 0;
    for i in 1..=n {
        s = if flag { s + i } else { s - 1 };
        s = (s * 3).rem_euclid(1000003);
    }
    s
}

#[test]
fn a_loop_trace_entered_with_a_boolean_leaves_its_head_to_one_that_runs() {
    let mut vm = vm();
    let f = function(&mut vm, LOOPED);
    for n in 50..60 {
        let r = int(&mut vm, f, &[Value::Bool(false), Value::Int(n)]);
        assert_eq!(r, looped_by_hand(false, n));
    }
    assert_eq!(
        vm.trace_compiled_count(),
        0,
        "a loop trace that reads a boolean on entry was compiled"
    );
    // the same path with a flag a trace can be entered with
    for n in 50..60 {
        let r = int(&mut vm, f, &[Value::Nil, Value::Int(n)]);
        assert_eq!(r, looped_by_hand(false, n), "flag nil, n {n}");
    }
    assert!(vm.trace_compiled_count() > 0, "no trace for the nil flag");
    assert!(
        vm.trace_dispatched_count() > 0,
        "the loop head stayed with a trace that cannot be entered"
    );
}
