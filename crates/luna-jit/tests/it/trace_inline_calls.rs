//! Traces that inline calls to functions of other prototypes: methods,
//! local and global functions, closures with upvalues, and the exits taken
//! inside the inlined functions. Each program runs under the interpreter
//! and under both trace tiers (and the move from one to the other); the
//! results must agree and the loop must have run in a trace.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn interp() -> Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

fn traced(tier: TraceTier, tier_up_at: u32) -> Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    // the method JIT would run whole functions instead of the interpreter
    // the trace recorder watches
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = 8;
    vm.jit.call_hot_threshold = 8;
    vm.set_trace_tier(tier);
    vm.set_trace_tier_up_at(tier_up_at);
    vm
}

/// `src` returns a function; the results of calling it `calls` times.
fn results(vm: &mut Vm, src: &str, calls: usize) -> Vec<String> {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    (0..calls)
        .map(|_| match vm.call_value(Value::Closure(f), &[]) {
            Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect()
}

fn show(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s.as_bytes())),
        v => format!("{v:?}"),
    }
}

const TIERS: [(&str, TraceTier, u32); 4] = [
    ("baseline", TraceTier::Baseline, 0),
    ("cranelift", TraceTier::Optimizing, 0),
    ("tier up at once", TraceTier::Auto, 1),
    ("tier up partway", TraceTier::Auto, 300),
];

/// Runs `src` in every tier against the interpreter; returns the trace
/// entries of each tier's Vm.
fn agree(src: &str, calls: usize) -> Vec<u64> {
    let want = results(&mut interp(), src, calls);
    TIERS
        .iter()
        .map(|&(name, tier, at)| {
            let mut vm = traced(tier, at);
            let got = results(&mut vm, src, calls);
            assert_eq!(got, want, "{name}");
            vm.trace_dispatched_count()
        })
        .collect()
}

fn assert_traced(src: &str, calls: usize) {
    for (k, n) in agree(src, calls).into_iter().enumerate() {
        assert!(n > 0, "{}: no trace ran", TIERS[k].0);
    }
}

/// The shape of `redis_lua_shape`'s method_dispatch: three methods found
/// through the metatable's `__index`, called with 0 and 1 results.
const METHODS: &str = r#"
local cls = {}
cls.__index = cls
function cls:get(k) return self.t[k] end
function cls:set(k, v) self.t[k] = v end
function cls:incr(k, by)
    self.t[k] = (self.t[k] or 0) + by
    return self.t[k]
end
return function()
    local o = setmetatable({t = {}}, cls)
    local last = 0
    for i = 1, 500 do
        o:set("k", i)
        local v = o:get("k")
        last = o:incr("k", 1) + v
    end
    return last
end
"#;

#[test]
fn method_calls_run_inlined_in_the_loop_trace() {
    assert_traced(METHODS, 6);
}

#[test]
fn a_method_loop_enters_its_trace_on_every_later_call() {
    // the closures are made again on each run of the chunk: the inlined
    // code is checked against their function, not their identity
    let src = r#"
return function()
    local cls = {}
    cls.__index = cls
    function cls:get(k) return self.t[k] end
    local o = setmetatable({t = {k = 3}}, cls)
    local s = 0
    for i = 1, 400 do s = s + o:get("k") + i end
    return s
end
"#;
    let mut vm = traced(TraceTier::Auto, 0);
    let want = results(&mut interp(), src, 10);
    assert_eq!(results(&mut vm, src, 10), want);
    assert!(
        vm.trace_dispatched_count() >= 8,
        "the loop trace was entered {} times in 10 calls",
        vm.trace_dispatched_count()
    );
}

#[test]
fn arguments_missing_and_extra_reach_the_callee_as_in_the_interpreter() {
    let src = r#"
local function two(a, b) if b == nil then return a else return a + b end end
local function one(a) return a * 2 end
return function()
    local s = 0
    for i = 1, 400 do
        s = s + two(i) + one(i, 7, 8)
    end
    return s
end
"#;
    assert_traced(src, 4);
}

#[test]
fn results_dropped_and_missing_are_as_in_the_interpreter() {
    let src = r#"
local t = {n = 0}
local function bump(x) t.n = t.n + x return t.n end
local function nothing(x) t.n = t.n - 1 end
return function()
    local s = 0
    for i = 1, 400 do
        bump(i)
        local v = nothing(i)
        if v == nil then s = s + 1 end
        s = s + bump(1)
    end
    return s, t.n
end
"#;
    assert_traced(src, 4);
}

#[test]
fn closures_of_one_function_read_their_own_upvalues() {
    let src = r#"
local function adder(k) return function(x) return x + k end end
local fs = {adder(1), adder(100)}
return function()
    local s = 0
    for i = 1, 600 do s = s + fs[i % 2 + 1](i) end
    return s
end
"#;
    assert_traced(src, 4);
}

#[test]
fn a_callee_reading_a_global_reads_it_through_its_own_environment() {
    let src = r#"
SCALE = 3
function scaled(x) return x * SCALE end
return function()
    local s = 0
    for i = 1, 500 do
        s = s + scaled(i)
        if i == 250 then SCALE = 5 end
    end
    return s
end
"#;
    agree(src, 4);
}

#[test]
fn an_upvalue_of_the_looping_frame_is_read_as_the_loop_left_it() {
    // `acc` lives in the frame the trace runs; the callee reads it while
    // the trace may hold its newest value only in a machine register
    let src = r#"
return function()
    local acc = 0
    local function get() return acc end
    local s = 0
    for i = 1, 500 do
        acc = i * 2
        s = s + get()
    end
    return s
end
"#;
    agree(src, 4);
}

#[test]
fn a_method_replaced_in_the_middle_of_the_loop_is_called() {
    let src = r#"
local cls = {}
cls.__index = cls
function cls:get() return 1 end
return function()
    local o = setmetatable({}, cls)
    local s = 0
    for i = 1, 600 do
        if i == 300 then cls.get = function() return 10 end end
        if i == 450 then o.get = function() return 100 end end
        s = s + o:get()
    end
    cls.get = function() return 1 end
    return s
end
"#;
    agree(src, 4);
}

#[test]
fn a_callee_turned_native_or_another_function_is_called() {
    let src = r#"
local f = function(x) return x + 1 end
return function()
    local s = 0
    local g = f
    for i = 1, 600 do
        if i == 200 then g = function(x) return x - 1 end end
        if i == 400 then g = math.abs end
        s = s + g(i)
    end
    return s
end
"#;
    agree(src, 4);
}

#[test]
fn an_exit_inside_an_inlined_function_resumes_in_it() {
    let src = r#"
local function f(x)
    if x > 300 then return x * 2 end
    return x
end
local function g(x) return f(x) + 1 end
return function()
    local s = 0
    for i = 1, 600 do s = s + g(i) end
    return s
end
"#;
    assert_traced(src, 4);
}

#[test]
fn an_error_inside_an_inlined_function_has_the_interpreter_traceback() {
    let src = r#"
local function f(t, x)
    return t.v + x
end
return function()
    local s, ok, err = 0
    for i = 1, 400 do
        local t = {v = i}
        if i == 350 then t = {} end
        ok, err = pcall(f, t, i)
        s = s + f(i == 350 and {v = 0} or t, i)
    end
    local _, msg = pcall(function() return f({}, 1) end)
    return s, msg
end
"#;
    agree(src, 3);
}

#[test]
fn nested_inlined_calls_return_to_the_right_frames() {
    let src = r#"
local P = {}
P.__index = P
function P:x() return self.px end
function P:len2() return self:x() * self:x() + self.py * self.py end
return function()
    local p = setmetatable({px = 3, py = 4}, P)
    local s = 0
    for i = 1, 500 do
        p.px = i % 7
        s = s + p:len2()
    end
    return s
end
"#;
    assert_traced(src, 4);
}

#[test]
fn a_loop_over_a_freshly_loaded_function_after_a_collection() {
    // the trace holds the inlined function's prototype: dropping every
    // closure of it and collecting must not free it under the trace
    let src = r#"
local function run(g)
    local s = 0
    for i = 1, 500 do s = s + g(i) end
    return s
end
return function()
    local a = run(load("return function(x) return x + 1 end")())
    collectgarbage()
    collectgarbage()
    local b = run(load("return function(x) return x + 2 end")())
    return a, b
end
"#;
    assert_traced(src, 4);
}
