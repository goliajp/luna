//! A register a looping trace writes and never reads (a local the loop
//! only assigns) is not lowered twice: it holds what it held on entry until
//! the call goes round its first back edge, a value of the kind the back
//! edge leaves after that, and every exit before its write restores
//! whichever of the two it holds. These run exits on both sides of the
//! first back edge, in both tiers, from a call nested inside another of the
//! same trace, and around collections, against the interpreter.

use luna_core::__private::{PassCount, ProtoTraceInternals};
use luna_jit::jit::trace::{ExitTag, TraceTier};
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

use super::trace_corpus::{interp, tiered};

/// `x` (register 2) is nil on entry and an integer after each pass; the
/// exit at `i == k` comes before its write.
const WRITE_ONLY: &str = r#"
return function(n, k)
    local x
    for i = 1, n do
        if i == k then return x, i end
        x = i * 3
    end
    return x
end
"#;

fn function_of(vm: &mut Vm, src: &str) -> Value {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    vm.call_value(Value::Closure(main), &[]).expect("chunk")[0]
}

fn show(vm: &mut Vm, f: Value, args: &[Value]) -> String {
    match vm.call_value(f, args) {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("error: {}", vm.error_display(&e)),
    }
}

/// Runs `src`'s function with each of `calls` under `vm` and the
/// interpreter, comparing every result; the function `vm` ran.
fn same(vm: &mut Vm, src: &str, calls: &[Vec<Value>]) -> Value {
    let mut it = interp(LuaVersion::Lua54);
    let (f, g) = (function_of(vm, src), function_of(&mut it, src));
    for (i, args) in calls.iter().enumerate() {
        assert_eq!(
            show(vm, f, args),
            show(&mut it, g, args),
            "call {i}: {args:?}"
        );
    }
    f
}

fn args(n: i64, k: i64) -> Vec<Value> {
    vec![Value::Int(n), Value::Int(k)]
}

/// The four exits: in the baseline tier (moving up never) and in the
/// optimizing one (from the first back edge on), in the first pass and
/// past a back edge.
#[test]
fn exits_before_a_written_only_register_is_written_restore_what_it_holds() {
    for at in [1_000_000, 1] {
        let mut vm = tiered(LuaVersion::Lua54, 4, TraceTier::Auto, at);
        let mut calls: Vec<Vec<Value>> = (0..6).map(|_| args(200, 0)).collect();
        calls.extend([args(200, 1), args(200, 7), args(200, 1), args(200, 150)]);
        let f = same(&mut vm, WRITE_ONLY, &calls);
        assert!(vm.trace_dispatched_count() > 0, "at {at}: no trace ran");
        if at == 1 {
            assert!(
                vm.trace_tiered_up_count() > 0,
                "the loop stayed in the baseline tier"
            );
        }
        let Value::Closure(c) = f else {
            unreachable!("the chunk returns a function")
        };
        let kinds: Vec<PassCount> = (c.proto.traces().iter())
            .map(|t| t.pass_count().clone())
            .collect();
        assert!(
            (kinds.iter())
                .any(|k| matches!(k, PassCount::PerCall(r) if r.contains(&(2, ExitTag::Int)))),
            "at {at}: x has no kind past the back edge: {kinds:?}"
        );
    }
}

/// A written-only register holding a table at the back edge is lowered as
/// a temporary instead, and the results stay the interpreter's.
#[test]
fn a_written_only_table_is_lowered_as_a_temporary() {
    let src = r#"
return function(n, k)
    local x, t
    for i = 1, n do
        if i == k then return x, t and t[1] end
        x = i * 3
        t = {i}
    end
    return x, t[1]
end
"#;
    for at in [1_000_000, 1] {
        let mut vm = tiered(LuaVersion::Lua54, 4, TraceTier::Auto, at);
        let calls: Vec<Vec<Value>> = [0, 0, 0, 0, 0, 1, 9, 1, 120]
            .into_iter()
            .map(|k| args(150, k))
            .collect();
        let _ = same(&mut vm, src, &calls);
        assert!(vm.trace_dispatched_count() > 0);
    }
}

/// The loop reads a field through `__index` on one call, whose function
/// runs the same loop (the same trace) many passes, before the outer call
/// leaves in its first pass: the inner call's passes are its own.
#[test]
fn a_call_nested_in_the_same_trace_does_not_count_for_the_outer_one() {
    let src = r#"
local f
local plain = {v = 1}
local meta = setmetatable({}, {__index = function() local x = f(60, 0, plain) return x end})
f = function(n, k, o)
    local x
    for i = 1, n do
        local v = o.v
        if i == k then return x, v end
        x = i * 3 + v
    end
    return x
end
return function(n, k, nested)
    return f(n, k, nested and meta or plain)
end
"#;
    for at in [1_000_000, 1] {
        let mut vm = tiered(LuaVersion::Lua54, 4, TraceTier::Auto, at);
        let mut calls: Vec<Vec<Value>> = (0..6)
            .map(|_| vec![Value::Int(100), Value::Int(0), Value::Bool(false)])
            .collect();
        for k in [1, 2, 5] {
            calls.push(vec![Value::Int(100), Value::Int(k), Value::Bool(true)]);
            calls.push(vec![Value::Int(100), Value::Int(k), Value::Bool(false)]);
        }
        let _ = same(&mut vm, src, &calls);
        assert!(vm.trace_dispatched_count() > 0);
    }
}

/// Collections fall due inside the loop (in the optimizing tier's loop
/// function too) while finalizers wait to run: an exit at every pass of the
/// first few, each in a call of its own.
#[test]
fn exits_around_collections_restore_what_the_register_holds() {
    let src = r#"
local sink
return function(n, k)
    for j = 1, 40 do setmetatable({}, {__gc = function() sink = j end}) end
    local x, t
    for i = 1, n do
        t = {i, i}
        if i == k then return x, t[2] end
        x = i * 3 + t[1]
    end
    return x
end
"#;
    for at in [1_000_000, 1, 37] {
        let mut vm = tiered(LuaVersion::Lua54, 4, TraceTier::Auto, at);
        let mut calls: Vec<Vec<Value>> = (0..6).map(|_| args(400, 0)).collect();
        calls.extend((1..=12).chain([100, 399]).map(|k| args(400, k)));
        let _ = same(&mut vm, src, &calls);
        assert!(vm.trace_dispatched_count() > 0);
    }
}
