//! A looping trace's temporaries: registers the body writes before it reads
//! them. The first pass finds in them whatever was there before the loop,
//! which the trace neither checks on entry nor writes back at an exit; the
//! later passes find what the pass before wrote, which an exit before the
//! body writes it again writes back.

use luna_jit::__private::VmJitState;
use luna_jit::Engine;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// `v`, kept from collection: the host holds it across calls.
fn pinned(vm: &mut Vm, v: Value) -> Value {
    vm.pin_host(v);
    v
}

/// Calls the function `src` returns with `args`, each list once.
fn calls(vm: &mut Vm, src: &str, args: &[Vec<Value>]) -> Vec<String> {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let f = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0];
    let f = pinned(vm, f);
    args.iter()
        .map(|a| match vm.call_value(f, a) {
            Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect()
}

/// A result by its content (a string's bytes, not its address).
fn show(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s.as_bytes())),
        v => format!("{v:?}"),
    }
}

fn jit(v: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_hot_threshold(4);
    vm
}

/// Each call stops the loop at another pass, the trace entered at the loop
/// head with what the interpreter left: exits before the body writes `last`
/// on the first pass (the stack's value stays) and on later ones (the pass
/// before's value is written back).
const EXIT_BEFORE_WRITE: &str = r#"
return function(t, start)
    local last = start
    for i = 1, #t + 1 do
        local v = t[i]
        if v == nil then break end
        last = v * 2
    end
    return last
end
"#;

#[test]
fn an_exit_before_the_write_leaves_what_the_interpreter_would() {
    for v in DIALECTS {
        let mut vm = jit(v);
        let mut want_vm = Vm::new(v);
        let mut args = Vec::new();
        for n in 0..40 {
            let t = vm.new_table().build();
            pinned(&mut vm, Value::Table(t));
            for i in 1..=n {
                vm.raw_set(t, Value::Int(i), Value::Int(i * 3)).unwrap();
            }
            args.push(vec![Value::Table(t), Value::Int(-1)]);
        }
        let got = calls(&mut vm, EXIT_BEFORE_WRITE, &args);
        let want_args: Vec<Vec<Value>> = (0..40)
            .map(|n| {
                let t = want_vm.new_table().build();
                pinned(&mut want_vm, Value::Table(t));
                for i in 1..=n {
                    want_vm
                        .raw_set(t, Value::Int(i), Value::Int(i * 3))
                        .unwrap();
                }
                vec![Value::Table(t), Value::Int(-1)]
            })
            .collect();
        let want = calls(&mut want_vm, EXIT_BEFORE_WRITE, &want_args);
        assert_eq!(got, want, "{v:?}");
        assert!(vm.jit().counters.closed > 0, "{v:?}: a trace closed");
    }
}

/// `x` is written on one way of a branch only: the trace leaves at the
/// branch (or takes the other way) in the passes that skip the write.
const WRITTEN_ON_ONE_WAY: &str = r#"
return function(n, m)
    local x, y = "x", "y"
    for i = 1, n do
        if i % m == 0 then
            x = i
        else
            y = i * 2
        end
        if i == n - 1 then x = x .. "" end
    end
    return x, y
end
"#;

#[test]
fn a_register_written_on_some_ways_keeps_each_pass_s_value() {
    for v in DIALECTS {
        let args: Vec<Vec<Value>> = (1..30)
            .flat_map(|n| (1..5).map(move |m| vec![Value::Int(n), Value::Int(m)]))
            .collect();
        let got = calls(&mut jit(v), WRITTEN_ON_ONE_WAY, &args);
        let want = calls(&mut Vm::new(v), WRITTEN_ON_ONE_WAY, &args);
        assert_eq!(got, want, "{v:?}");
    }
}

/// The loop's temporaries (`k`, and the concatenation's and the length's
/// operands) take the registers the `do` block's locals held, which hold
/// `s` when the loop starts.
const STALE: &str = r#"
return function(s)
    do local a, b, c, d, e, f, g, h = s, s, s, s, s, s, s, s end
    local n = 0
    for i = 1, 300 do
        local k = "item:" .. i
        n = n + #k
    end
    return n
end
"#;

/// The trace a Vm compiled is installed in another of the engine whatever
/// the temporaries' registers hold when the loop starts there: a string,
/// nil (as a collection leaves the stack above its top) or a table.
#[test]
fn what_a_temporary_held_before_the_loop_does_not_stop_its_trace_being_shared() {
    let s = |vm: &mut Vm, x: &str| match x {
        "nil" => Value::Nil,
        "table" => {
            let t = Value::Table(vm.new_table().build());
            pinned(vm, t)
        }
        _ => {
            let s = Value::Str(vm.intern(x));
            pinned(vm, s)
        }
    };
    for v in DIALECTS {
        let engine = Engine::new();
        let mut first = engine.new_vm(v);
        first.set_jit_enabled(false);
        first.set_trace_hot_threshold(8);
        let a = s(&mut first, "str");
        let want = calls(&mut first, STALE, &[vec![a]]);
        assert!(
            first.jit().counters.closed > 0,
            "{v:?}: the first Vm compiled"
        );
        for held in ["nil", "table", "other"] {
            let mut vm = engine.new_vm(v);
            vm.set_jit_enabled(false);
            vm.set_trace_hot_threshold(8);
            let a = s(&mut vm, held);
            assert_eq!(calls(&mut vm, STALE, &[vec![a]]), want, "{v:?} {held}");
            let c = &vm.jit().counters;
            assert_eq!(c.closed, 0, "{v:?} {held}: recorded again");
            assert!(c.adopted > 0, "{v:?} {held}: nothing installed");
        }
    }
}

/// `x` is written on one way of an inner branch of one way of an outer
/// one: only the passes that take both write it, the others keep the
/// value of the pass that last did.
const WRITTEN_IN_A_NESTED_BRANCH: &str = r#"
return function(n, m, k)
    local x, y = "x", 0
    for i = 1, n do
        if i % m == 0 then
            if i % k == 0 then x = i .. "" else y = y + 1 end
        else
            y = y + i
        end
        if i == n - 1 then x = x .. "!" end
    end
    return x, y
end
"#;

#[test]
fn a_register_written_in_a_nested_branch_keeps_each_pass_s_value() {
    for v in DIALECTS {
        let args: Vec<Vec<Value>> = (1..30)
            .flat_map(|n| {
                (1..4).flat_map(move |m| {
                    (1..4).map(move |k| vec![Value::Int(n), Value::Int(m), Value::Int(k)])
                })
            })
            .collect();
        let mut vm = jit(v);
        let got = calls(&mut vm, WRITTEN_IN_A_NESTED_BRANCH, &args);
        let want = calls(&mut Vm::new(v), WRITTEN_IN_A_NESTED_BRANCH, &args);
        assert_eq!(got, want, "{v:?}");
        assert!(vm.jit().counters.closed > 0, "{v:?}: a trace closed");
    }
}

/// A temporary holds a string the pass made while the next allocation may
/// run a collection step: on the later passes the trace's copy of it is
/// a root, and an exit writes back a live string.
const STRING_ACROSS_A_STEP: &str = r#"
return function(n)
    local total, last = 0, nil
    for i = 1, n do
        local s = "k" .. i
        local t = {s, i}
        if t[2] % 9 == 0 then collectgarbage("step") end
        total = total + #s + #t[1]
        last = s
    end
    return total, last
end
"#;

#[test]
fn a_temporary_string_survives_a_collection_in_the_loop() {
    for v in DIALECTS {
        let args: Vec<Vec<Value>> = (0..60).map(|n| vec![Value::Int(n * 7)]).collect();
        let mut vm = jit(v);
        let got = calls(&mut vm, STRING_ACROSS_A_STEP, &args);
        let want = calls(&mut Vm::new(v), STRING_ACROSS_A_STEP, &args);
        assert_eq!(got, want, "{v:?}");
        assert!(vm.jit().counters.closed > 0, "{v:?}: a trace closed");
    }
}
