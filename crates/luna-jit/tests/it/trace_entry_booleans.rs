//! Booleans as a trace entry type: a trace that reads, on entry, a register
//! holding `false` or `true` is compiled and entered with either value, the
//! branches on it guarded. A register holding a value no trace is entered
//! with (a coroutine here) still keeps the trace from being compiled and
//! leaves its head free for a later recording.

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
    s = s % 1000003 s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2
    s = s % 1000003 s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2
    s = s % 1000003 s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2
    s = s % 1000003 s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2
    s = s % 1000003 s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2
    s = s % 1000003 s = s + a  s = s * 3  s = s - 7  s = s + a  s = s * 2
    return s
end
"#;

fn called_by_hand(flag: bool, a: i64) -> i64 {
    let mut s = i64::from(flag);
    s += a;
    s *= 3;
    s -= 7;
    s += a;
    s *= 2;
    s -= a;
    s += 11;
    s *= 3;
    s -= a;
    s += 5;
    s *= 2;
    s -= 1;
    for _ in 0..6 {
        s = s.rem_euclid(1000003);
        s += a;
        s *= 3;
        s -= 7;
        s += a;
        s *= 2;
    }
    s
}

#[test]
fn a_call_trace_entered_with_a_boolean_runs_for_both_values() {
    let mut vm = vm();
    let f = function(&mut vm, CALLED);
    for k in 0..20 {
        let r = int(&mut vm, f, &[Value::Bool(false), Value::Int(k)]);
        assert_eq!(r, called_by_hand(false, k));
    }
    assert!(
        vm.trace_compiled_count() > 0,
        "no trace for the boolean entry"
    );
    let entered = vm.trace_dispatched_count();
    assert!(entered > 0, "the boolean entry did not run the trace");
    for k in 0..20 {
        let r = int(&mut vm, f, &[Value::Bool(true), Value::Int(k)]);
        assert_eq!(r, called_by_hand(true, k), "flag true, a {k}");
    }
    assert!(
        vm.trace_dispatched_count() > entered,
        "a true flag did not enter the trace"
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
fn a_loop_trace_entered_with_a_boolean_runs_for_both_values() {
    let mut vm = vm();
    let f = function(&mut vm, LOOPED);
    for n in 50..60 {
        let r = int(&mut vm, f, &[Value::Bool(false), Value::Int(n)]);
        assert_eq!(r, looped_by_hand(false, n));
    }
    assert!(vm.trace_compiled_count() > 0, "no trace for the loop");
    let entered = vm.trace_dispatched_count();
    assert!(entered > 0, "the loop ran no trace");
    // the true flag takes the `then` branch, whose jump over the `else`
    // the trace follows
    for n in 50..60 {
        let r = int(&mut vm, f, &[Value::Bool(true), Value::Int(n)]);
        assert_eq!(r, looped_by_hand(true, n), "flag true, n {n}");
    }
    assert!(
        vm.trace_dispatched_count() > entered,
        "a true flag ran no trace"
    );
}

#[test]
fn booleans_made_compared_and_stored_in_a_loop_trace() {
    let src = r#"
return function(n)
    local on, t, s = false, {}, 0
    for i = 1, n do
        if i % 5 == 0 then on = not on end
        local big = i > 40
        if on == true then s = s + 2 end
        if big then s = s + 1 end
        t.on = on
        if not t.on then s = s + 3 end
    end
    return s
end
"#;
    let mut vm = vm();
    let f = function(&mut vm, src);
    let mut it = luna_jit::new_with_jit(LuaVersion::Lua54);
    it.set_jit_enabled(false);
    it.set_trace_jit_enabled(false);
    let g = function(&mut it, src);
    for n in [10, 60, 100, 101, 7] {
        assert_eq!(
            int(&mut vm, f, &[Value::Int(n)]),
            int(&mut it, g, &[Value::Int(n)]),
            "n {n}"
        );
    }
    assert!(vm.trace_dispatched_count() > 0, "the loop ran no trace");
}

/// A register holding a coroutine on entry: a trace reading it is never
/// entered, so it is not compiled, and the head stays free.
#[test]
fn a_loop_trace_entered_with_a_coroutine_leaves_its_head_to_one_that_runs() {
    let mut vm = vm();
    let f = function(&mut vm, LOOPED);
    let co = vm
        .eval("return coroutine.create(function() end)")
        .expect("coroutine")[0];
    for n in 50..60 {
        let r = int(&mut vm, f, &[co, Value::Int(n)]);
        assert_eq!(r, looped_by_hand(true, n));
    }
    assert_eq!(
        vm.trace_compiled_count(),
        0,
        "a loop trace that reads a coroutine on entry was compiled"
    );
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
