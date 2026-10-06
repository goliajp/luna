//! Vms built through one `Engine` share the traces they compile: a second
//! Vm running code of the same content installs them instead of recording
//! and compiling, and gets the same results as the interpreter with its
//! own globals, upvalues and strings.

use luna_jit::Engine;
use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// A table read and written through string keys in a loop, with a branch
/// and a folded `math.min` (the token bucket of the benchmarks).
const TOKEN: &str = r#"
return function()
    local bucket = { tokens = 1000, last = 0, rate = 100 }
    local now, refilled = 1, 0
    for i = 1, 600 do
        local elapsed = now - bucket.last
        local refill = elapsed * bucket.rate
        if refill > 0 then
            bucket.tokens = math.min(1000, bucket.tokens + refill)
            bucket.last = now
            refilled = refilled + 1
        end
        if bucket.tokens >= 1 then bucket.tokens = bucket.tokens - 1 end
        now = now + 1
    end
    return bucket.tokens, refilled
end
"#;

/// Method calls through `__index`, inlined into the loop's trace from the
/// methods' own prototypes.
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
    for i = 1, 800 do
        o:set("k", i)
        local v = o:get("k")
        last = o:incr("k", 1) + v
    end
    return last
end
"#;

/// Self-recursion: a call-triggered trace whose hot exits get side traces.
const SIDE_EXITS: &str = r#"
local function f(n)
    if n < 2 then return n end
    return f(n - 1) + f(n - 2)
end
return function() return f(18) end
"#;

/// A generic `for` over `ipairs` and string concatenation.
const ITER: &str = r#"
return function()
    local t = {}
    for i = 1, 300 do t[i] = i * 3 end
    local s, parts = 0, {}
    for _, v in ipairs(t) do
        s = s + v
        if v % 50 == 0 then parts[#parts + 1] = "p" .. v end
    end
    return s, table.concat(parts, ",")
end
"#;

const PROGRAMS: [(&str, &str); 4] = [
    ("token", TOKEN),
    ("methods", METHODS),
    ("side exits", SIDE_EXITS),
    ("iter", ITER),
];

fn interp(v: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

/// A Vm of `engine` that traces sooner, without the method JIT (which would
/// run whole functions instead of the interpreter the recorder watches).
fn shared(engine: &Engine, v: LuaVersion) -> Vm {
    let mut vm = engine.new_vm(v);
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = 8;
    vm.jit.call_hot_threshold = 8;
    vm
}

/// What running a program in a Vm did.
#[derive(Debug)]
struct Run {
    results: Vec<String>,
    compiled: u64,
    adopted: u64,
    dispatched: u64,
    /// Machine code generated on this thread meanwhile.
    codegen: u64,
}

/// `src` returns a function; calls it `calls` times in `vm`.
fn run(vm: &mut Vm, src: &str, calls: usize) -> Run {
    let codegen = luna_jit::jit_backend::trace::trace_codegen_count();
    let compiled = vm.trace_compiled_count();
    let adopted = vm.trace_adopted_count();
    let dispatched = vm.trace_dispatched_count();
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    let results = (0..calls)
        .map(|_| match vm.call_value(Value::Closure(f), &[]) {
            Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect();
    Run {
        results,
        compiled: vm.trace_compiled_count() - compiled,
        adopted: vm.trace_adopted_count() - adopted,
        dispatched: vm.trace_dispatched_count() - dispatched,
        codegen: luna_jit::jit_backend::trace::trace_codegen_count() - codegen,
    }
}

fn show(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s.as_bytes())),
        v => format!("{v:?}"),
    }
}

fn assert_adopted_only(name: &str, r: &Run, want: &[String]) {
    assert_eq!(r.results, want, "{name}: results");
    assert_eq!(
        r.compiled, 0,
        "{name}: the second Vm compiled traces: {r:?}"
    );
    assert_eq!(r.codegen, 0, "{name}: the second Vm generated code: {r:?}");
    assert!(r.adopted > 0, "{name}: nothing was installed: {r:?}");
    assert!(r.dispatched > 0, "{name}: no installed trace ran: {r:?}");
}

#[test]
fn a_second_vm_compiles_nothing() {
    for (name, src) in PROGRAMS {
        let want = run(&mut interp(LuaVersion::Lua54), src, 3).results;
        let engine = Engine::new();
        let a = run(&mut shared(&engine, LuaVersion::Lua54), src, 3);
        assert_eq!(a.results, want, "{name}: first Vm");
        assert!(a.compiled > 0, "{name}: the first Vm compiled nothing");
        let b = run(&mut shared(&engine, LuaVersion::Lua54), src, 3);
        assert_adopted_only(name, &b, &want);
    }
}

#[test]
fn side_traces_come_with_their_parent() {
    let engine = Engine::new();
    let mut a = shared(&engine, LuaVersion::Lua54);
    run(&mut a, SIDE_EXITS, 3);
    assert!(a.trace_side_trace_compiled_count() > 0, "no side trace");
    let mut b = shared(&engine, LuaVersion::Lua54);
    let r = run(&mut b, SIDE_EXITS, 3);
    assert_eq!(r.compiled, 0);
    assert_eq!(
        b.trace_adopted_count(),
        a.trace_compiled_count(),
        "each trace installed once"
    );
    assert_eq!(
        b.trace_side_trace_compiled_count(),
        a.trace_side_trace_compiled_count(),
        "the side traces were not linked into the installed parent"
    );
}

/// The same code loaded again in the same Vm takes the traces it compiled
/// the first time.
#[test]
fn reloading_code_in_one_vm_compiles_it_once() {
    let engine = Engine::new();
    let mut vm = shared(&engine, LuaVersion::Lua54);
    let want = run(&mut interp(LuaVersion::Lua54), TOKEN, 2).results;
    let first = run(&mut vm, TOKEN, 2);
    assert!(first.compiled > 0);
    let again = run(&mut vm, TOKEN, 2);
    assert_adopted_only("reload", &again, &want);
}

#[test]
fn the_optimizing_tier_carries_over() {
    let engine = Engine::new();
    let want = run(&mut interp(LuaVersion::Lua54), TOKEN, 4).results;
    let mut a = shared(&engine, LuaVersion::Lua54);
    a.set_trace_tier_up_at(1);
    run(&mut a, TOKEN, 4);
    assert!(a.trace_tiered_up_count() > 0, "nothing moved to Cranelift");
    let mut b = shared(&engine, LuaVersion::Lua54);
    b.set_trace_tier_up_at(1);
    let r = run(&mut b, TOKEN, 4);
    assert_adopted_only("tier 2", &r, &want);
    assert_eq!(
        b.trace_tiered_up_count(),
        0,
        "the installed code was not Cranelift's"
    );
}

/// A Vm moving a trace to Cranelift after it installed another Vm's baseline
/// code gives the Cranelift code to the engine, and a third Vm takes it.
#[test]
fn the_optimizing_tier_compiled_by_a_later_vm_is_shared_too() {
    // `tier_up_at` is part of what must agree, so every Vm keeps it
    const AT: u32 = 3000;
    let engine = Engine::new();
    let want = run(&mut interp(LuaVersion::Lua54), TOKEN, 6).results;
    let mut a = shared(&engine, LuaVersion::Lua54);
    a.set_trace_tier_up_at(AT);
    run(&mut a, TOKEN, 1);
    assert_eq!(a.trace_tiered_up_count(), 0, "one call moved the loop");
    let mut b = shared(&engine, LuaVersion::Lua54);
    b.set_trace_tier_up_at(AT);
    let r = run(&mut b, TOKEN, 6);
    assert_eq!(r.results, want);
    assert_eq!(r.compiled, 0);
    assert!(r.adopted > 0);
    assert!(
        b.trace_tiered_up_count() > 0,
        "the installed trace never moved"
    );
    let mut c = shared(&engine, LuaVersion::Lua54);
    c.set_trace_tier_up_at(AT);
    let r = run(&mut c, TOKEN, 6);
    assert_adopted_only("third", &r, &want);
    assert_eq!(
        c.trace_tiered_up_count(),
        0,
        "the third Vm compiled Cranelift code"
    );
}

#[test]
fn the_first_vm_may_be_gone() {
    let engine = Engine::new();
    for (name, src) in PROGRAMS {
        let want = run(&mut interp(LuaVersion::Lua54), src, 2).results;
        {
            let mut a = shared(&engine, LuaVersion::Lua54);
            run(&mut a, src, 2);
        }
        // reuse the freed memory: the dropped Vm's strings and functions
        // now hold other objects
        let mut other = luna_jit::new_with_jit(LuaVersion::Lua54);
        other
            .eval("local t = {} for i = 1, 20000 do t[i] = {('s' .. i):rep(3)} end")
            .expect("churn");
        drop(other);
        let b = run(&mut shared(&engine, LuaVersion::Lua54), src, 2);
        assert_adopted_only(name, &b, &want);
    }
}

/// Reads a global, a field of a global table and an upvalue set from a
/// global when the chunk runs.
const GLOBALS: &str = r#"
local k = base
return function()
    local s = 0
    for i = 1, 400 do s = s + k * i + G.v end
    return s
end
"#;

fn with_globals(vm: &mut Vm, base: i64, v: i64) {
    vm.eval(&format!("base = {base}; G = {{ v = {v} }}"))
        .expect("globals");
}

#[test]
fn each_vm_reads_its_own_globals_and_upvalues() {
    let engine = Engine::new();
    let mut a = shared(&engine, LuaVersion::Lua54);
    with_globals(&mut a, 2, 1);
    let ra = run(&mut a, GLOBALS, 2);
    let mut b = shared(&engine, LuaVersion::Lua54);
    with_globals(&mut b, 5, 7);
    let mut ib = interp(LuaVersion::Lua54);
    with_globals(&mut ib, 5, 7);
    let want = run(&mut ib, GLOBALS, 2).results;
    let rb = run(&mut b, GLOBALS, 2);
    assert_ne!(
        ra.results, want,
        "the two Vms should compute different sums"
    );
    assert_adopted_only("globals", &rb, &want);
}

#[test]
fn strings_interned_in_another_order() {
    let engine = Engine::new();
    for (name, src) in PROGRAMS {
        let want = run(&mut interp(LuaVersion::Lua54), src, 2).results;
        run(&mut shared(&engine, LuaVersion::Lua54), src, 2);
        let mut b = shared(&engine, LuaVersion::Lua54);
        // other strings first, and the program's keys made at other
        // addresses than in the first Vm
        b.eval(
            "local t = {} for i = 1, 3000 do t['x' .. i] = i end \
             junk = {'rate', 'last', 'tokens', 'get', 'set', 'incr', 't', 'k', 'p'}",
        )
        .expect("strings");
        let r = run(&mut b, src, 2);
        assert_adopted_only(name, &r, &want);
    }
}

#[test]
fn dialects_never_share() {
    let engine = Engine::new();
    run(&mut shared(&engine, LuaVersion::Lua54), TOKEN, 2);
    for v in [LuaVersion::Lua53, LuaVersion::Lua55] {
        let want = run(&mut interp(v), TOKEN, 2).results;
        let r = run(&mut shared(&engine, v), TOKEN, 2);
        assert_eq!(r.results, want, "{v:?}");
        assert_eq!(r.adopted, 0, "{v:?} took a 5.4 trace");
    }
    // a second 5.5 Vm takes what the first compiled
    let r = run(&mut shared(&engine, LuaVersion::Lua55), TOKEN, 2);
    assert!(r.adopted > 0 && r.compiled == 0, "{r:?}");
}

#[test]
fn trace_settings_never_mix() {
    let engine = Engine::new();
    let mut a = shared(&engine, LuaVersion::Lua54);
    a.set_trace_tier(TraceTier::Baseline);
    run(&mut a, TOKEN, 2);
    let mut b = shared(&engine, LuaVersion::Lua54);
    b.set_trace_tier(TraceTier::Optimizing);
    let r = run(&mut b, TOKEN, 2);
    assert_eq!(r.adopted, 0, "a Cranelift-only Vm took baseline code");
    assert!(r.compiled > 0);
    let mut c = shared(&engine, LuaVersion::Lua54);
    c.set_trace_tier(TraceTier::Optimizing);
    assert_eq!(run(&mut c, TOKEN, 2).compiled, 0);
}

/// A loop over a value passed in: integers in one Vm, floats in the next.
const ARG: &str = r#"
return function(x)
    local s = x
    for i = 1, 500 do s = s + x end
    return s
end
"#;

fn run_arg(vm: &mut Vm, x: Value) -> (Value, u64, u64) {
    let (c, a) = (vm.trace_compiled_count(), vm.trace_adopted_count());
    let main = vm.load(ARG.as_bytes(), b"=t").expect("load");
    let f = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0];
    let mut r = Value::Nil;
    for _ in 0..3 {
        r = vm.call_value(f, &[x]).expect("call")[0];
    }
    (
        r,
        vm.trace_compiled_count() - c,
        vm.trace_adopted_count() - a,
    )
}

#[test]
fn other_entry_types_get_a_variant_of_their_own() {
    let engine = Engine::new();
    let (r, c, _) = run_arg(&mut shared(&engine, LuaVersion::Lua54), Value::Int(3));
    assert!(matches!(r, Value::Int(1503)), "{r:?}");
    assert!(c > 0);
    let (r, c, a) = run_arg(&mut shared(&engine, LuaVersion::Lua54), Value::Float(0.5));
    assert!(matches!(r, Value::Float(f) if f == 250.5), "{r:?}");
    assert!(c > 0, "floats took the integer trace");
    assert_eq!(a, 0);
    let (r, c, a) = run_arg(&mut shared(&engine, LuaVersion::Lua54), Value::Float(0.5));
    assert!(matches!(r, Value::Float(f) if f == 250.5), "{r:?}");
    assert_eq!(c, 0);
    assert!(a > 0);
}

/// Without an engine every Vm compiles its own traces.
#[test]
fn vms_without_an_engine_share_nothing() {
    for _ in 0..2 {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(false);
        vm.jit.trace_hot_threshold = 8;
        let r = run(&mut vm, TOKEN, 2);
        assert!(r.compiled > 0);
        assert_eq!(r.adopted, 0);
    }
}

/// A function inlined from another chunk is found in that chunk.
#[test]
fn a_function_inlined_from_another_chunk_is_found() {
    const LIB: &str = "function scale(x) return x * 3 + 1 end";
    const USE: &str = r#"
return function()
    local s = 0
    for i = 1, 500 do s = s + scale(i) end
    return s
end
"#;
    let engine = Engine::new();
    let mut want_vm = interp(LuaVersion::Lua54);
    want_vm.eval(LIB).expect("lib");
    let want = run(&mut want_vm, USE, 2).results;
    let mut a = shared(&engine, LuaVersion::Lua54);
    a.eval(LIB).expect("lib");
    run(&mut a, USE, 2);
    let mut b = shared(&engine, LuaVersion::Lua54);
    b.eval(LIB).expect("lib");
    let r = run(&mut b, USE, 2);
    assert_adopted_only("other chunk", &r, &want);
    // the library loaded from other text compiling to the same code
    let mut c = shared(&engine, LuaVersion::Lua54);
    c.eval("\n\n  function scale(x)\n return x * 3 + 1 end -- moved")
        .expect("lib");
    let r = run(&mut c, USE, 2);
    assert_adopted_only("same code, other lines", &r, &want);
}

mod failures;
mod functions;
mod threads;
