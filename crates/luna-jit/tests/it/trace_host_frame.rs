//! A recording whose frame returns to the host, or whose thread switches
//! to another coroutine, stops there: the next op the interpreter runs
//! belongs to another call (the host calling the function again, or a
//! library function calling back into Lua) or another thread, and
//! recording it would put two executions' ops in one trace. A call trace
//! of a function the host calls again and again is the exception: the next
//! call starts at the trace's head, where the recording closes.

use luna_core::__private::VmJitState;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

// the comparator's loop gets hot on its last pass in some call: the
// recording runs on past the loop and returns to `table.sort`, which calls
// the comparator again
const SORT: &str = r#"
local t = {}
for i = 1, 200 do t[i] = (i * 7919) % 211 end
local calls = 0
table.sort(t, function(a, b)
    local x = 0
    for i = 1, 3 do x = x + i end
    calls = calls + x
    return a < b
end)
local s = 0
for i = 1, #t do s = s * 31 % 1000003 + t[i] end
return s, calls > 0
"#;

fn run(v: LuaVersion, src: &str, jit: bool, hot: u32, cause: &str) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm.set_trace_hot_threshold(hot);
    vm.set_call_hot_threshold(hot);
    let r: Vec<Value> = vm.eval(src).unwrap();
    let left = vm
        .trace_close_cause_counts()
        .get(cause)
        .copied()
        .unwrap_or(0);
    (format!("{r:?}"), left)
}

#[test]
fn a_recording_stops_when_its_frame_returns_to_a_library_function() {
    for v in DIALECTS {
        let (want, _) = run(v, SORT, false, 1, "left-start-frame");
        let mut left = 0;
        for hot in [1, 2, 3, 5] {
            let (got, n) = run(v, SORT, true, hot, "left-start-frame");
            assert_eq!(got, want, "{v:?} hot {hot}");
            left += n;
        }
        assert!(left > 0, "{v:?}: no recording left its frame");
    }
}

// the loop gets hot on the last pass of some call and the recording
// returns with the function to the host, which calls it again
const HOST: &str = "return function(n) local s = 0 for i = 1, n do s = s + i end return s end";

fn host_calls(v: LuaVersion, jit: bool) -> (Vec<String>, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm.set_trace_hot_threshold(3);
    let f = vm.eval(HOST).unwrap().remove(0);
    let out = (0..20)
        .map(|k| format!("{:?}", vm.call_value(f, &[Value::Int(1 + k % 4)]).unwrap()))
        .collect();
    let left = vm
        .trace_close_cause_counts()
        .get("left-start-frame")
        .copied()
        .unwrap_or(0);
    (out, left)
}

#[test]
fn a_recording_stops_when_its_frame_returns_to_the_host() {
    for v in DIALECTS {
        let (want, _) = host_calls(v, false);
        let (got, left) = host_calls(v, true);
        assert_eq!(got, want, "{v:?}");
        assert!(left > 0, "{v:?}: no recording left its frame");
    }
}

// the loop's recording resumes a coroutine on every pass: the ops that run
// next are the coroutine's
const CORO: &str = r#"
local gen = coroutine.wrap(function()
    local i = 0
    while true do i = i + 1 coroutine.yield(i * 3 % 7) end
end)
local s = 0
for i = 1, 300 do s = s + gen() + i end
return s
"#;

#[test]
fn a_recording_stops_when_its_thread_switches_to_a_coroutine() {
    for v in DIALECTS {
        let (want, _) = run(v, CORO, false, 1, "left-start-thread");
        let mut left = 0;
        for hot in [1, 2, 3, 5] {
            let (got, n) = run(v, CORO, true, hot, "left-start-thread");
            assert_eq!(got, want, "{v:?} hot {hot}");
            left += n;
        }
        assert!(left > 0, "{v:?}: no recording left its thread");
    }
}

// a function with no loop, called by the host in a loop: its call trace
// closes at its head when the host calls it again, and its body is long
// enough for the trace to be dispatched
const CALLED: &str = r#"
return function(a, b)
    local s = a * 3 + b
    s = s % 1009 + a  s = s * 5 - b  s = s % 1013 + a  s = s * 7 - b
    s = s % 1019 + a  s = s * 3 - b  s = s % 1021 + a  s = s * 5 - b
    s = s % 1031 + a  s = s * 7 - b  s = s % 1033 + a  s = s * 3 - b
    s = s % 1039 + a  s = s * 5 - b  s = s % 1049 + a  s = s * 7 - b
    s = s % 1051 + a  s = s * 3 - b  s = s % 1061 + a  s = s * 5 - b
    s = s % 1063 + a  s = s * 7 - b  s = s % 1069 + a  s = s * 3 - b
    s = s % 1087 + a  s = s * 5 - b  s = s % 1091 + a  s = s * 7 - b
    s = s % 1093 + a  s = s * 3 - b  s = s % 1097 + a  s = s * 5 - b
    return s
end
"#;

/// The results of 40 host calls of [`CALLED`], the traces compiled and
/// dispatched, and the recordings stopped for leaving their frame (a side
/// trace's, which begins at an exit, still is).
fn host_loop(v: LuaVersion, jit: bool) -> (Vec<String>, u64, u64, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm.set_call_hot_threshold(2);
    vm.set_trace_hot_threshold(2);
    let f = vm.eval(CALLED).unwrap().remove(0);
    let out = (0..40)
        .map(|k| {
            format!(
                "{:?}",
                vm.call_value(f, &[Value::Int(k), Value::Int(k % 7)])
                    .unwrap()
            )
        })
        .collect();
    let left = vm
        .trace_close_cause_counts()
        .get("left-start-frame")
        .copied()
        .unwrap_or(0);
    let (compiled, dispatched) = (vm.trace_compiled_count(), vm.trace_dispatched_count());
    (out, compiled, dispatched, left)
}

#[test]
fn a_function_the_host_calls_in_a_loop_gets_a_call_trace() {
    for v in DIALECTS {
        let (want, ..) = host_loop(v, false);
        let (got, compiled, dispatched, left) = host_loop(v, true);
        assert_eq!(got, want, "{v:?}");
        let counts = format!("{compiled} compiled, {dispatched} dispatched, {left} left");
        assert!(compiled > 0, "{v:?}: no trace compiled ({counts})");
        assert!(dispatched > 0, "{v:?}: the trace never ran ({counts})");
    }
}
