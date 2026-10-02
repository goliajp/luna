//! Loops the trace JIT runs natively: each loop of a function gets hot on
//! its own, table reads are typed by what the recording saw, field and
//! array slots are read and written in place, `string.sub` and short
//! string constants stay in the trace, and a comparison's short other
//! branch is taken inside the trace. Each test checks the trace really ran
//! (and how often it had to be entered) and that the results match the
//! interpreter's.

use luna_jit::runtime::{Gc, LuaClosure, Value};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// A Vm with the trace JIT only: the method JIT would take some of these
/// functions whole.
fn trace_vm() -> Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm
}

fn interp_vm() -> Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

/// Load `src` (a chunk returning a function) and return that function.
fn function(vm: &mut Vm, src: &str) -> Gc<LuaClosure> {
    let main = vm.load(src.as_bytes(), b"=cov").expect("load");
    let r = vm.call_value(Value::Closure(main), &[]).expect("run chunk");
    match r[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    }
}

fn call(vm: &mut Vm, f: Gc<LuaClosure>, args: &[Value]) -> Vec<Value> {
    vm.call_value(Value::Closure(f), args).expect("call")
}

/// Values a Lua script can compare: numbers and strings as text.
fn show(vs: &[Value]) -> String {
    vs.iter()
        .map(|v| match v {
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Run `f(args)` `times` times under the trace JIT and once more under the
/// interpreter; the last results must agree. Returns the trace Vm, the
/// function and the number of trace entries the last call made.
fn run_both(src: &str, args: &[Value], times: usize) -> (Vm, Gc<LuaClosure>, u64) {
    let mut vm = trace_vm();
    let f = function(&mut vm, src);
    let mut last = Vec::new();
    for _ in 0..times {
        last = call(&mut vm, f, args);
    }
    let before = vm.trace_dispatched_count();
    let again = call(&mut vm, f, args);
    let entries = vm.trace_dispatched_count() - before;
    assert_eq!(show(&again), show(&last));
    let mut iv = interp_vm();
    let fi = function(&mut iv, src);
    let want = call(&mut iv, fi, args);
    assert_eq!(
        show(&again),
        show(&want),
        "trace JIT and interpreter differ"
    );
    (vm, f, entries)
}

fn dispatchable_heads(f: Gc<LuaClosure>) -> Vec<u32> {
    let mut heads: Vec<u32> = f
        .proto
        .traces
        .borrow()
        .iter()
        .filter(|t| t.dispatchable)
        .map(|t| t.head_pc)
        .collect();
    heads.sort_unstable();
    heads
}

#[test]
fn every_loop_of_a_function_gets_its_own_trace() {
    let src = "
        return function(n)
            local s = 0
            for i = 1, n do s = s + i end
            local t = 0
            for i = 1, n do t = t + i * 3 end
            local u, j = 0, 0
            while j < n do j = j + 1 u = u + j end
            return s, t, u
        end";
    let (_vm, f, entries) = run_both(src, &[Value::Int(1000)], 3);
    assert_eq!(
        dispatchable_heads(f).len(),
        3,
        "{:?}",
        dispatchable_heads(f)
    );
    // one entry per loop: each runs all its iterations inside
    assert_eq!(entries, 3);
}

#[test]
fn a_table_read_is_typed_by_the_value_the_recording_saw() {
    // the read's next op does not use it, so nothing after it tells its type
    let src = "
        local w = {}
        for i = 1, 300 do w[i] = i end
        return function(lim)
            local i = 1
            while i <= #w and w[i] < lim - 1 do i = i + 1 end
            return i
        end";
    let (_vm, f, entries) = run_both(src, &[Value::Int(250)], 3);
    assert_eq!(dispatchable_heads(f).len(), 1);
    assert_eq!(entries, 1);
}

#[test]
fn a_present_field_is_read_and_written_in_its_slot_past_a_metatable() {
    // the field is there, so neither `__index` nor `__newindex` applies:
    // the loop stays in the trace although the table has a metatable
    let src = "
        local o = setmetatable({n = 0, step = 2}, {
            __index = function() error('no __index for present keys') end,
            __newindex = function() error('no __newindex for present keys') end,
        })
        return function(k)
            o.n = 0
            for i = 1, k do o.n = o.n + o.step end
            return o.n
        end";
    let (_vm, f, entries) = run_both(src, &[Value::Int(1000)], 3);
    assert_eq!(dispatchable_heads(f).len(), 1);
    assert_eq!(entries, 1);
}

#[test]
fn array_reads_writes_and_lengths_stay_in_the_trace() {
    // `src` has a metatable, whose `__index` the present slots never reach
    let src = "
        local src = setmetatable({}, {__index = function() return 0 end})
        for i = 1, 200 do src[i] = i * 7 end
        return function()
            local out = {}
            for j = 1, #src do out[#out + 1] = src[j] end
            local s = 0
            for j = 1, #out do s = s + out[j] end
            return #out, s
        end";
    let (_vm, f, entries) = run_both(src, &[], 3);
    assert_eq!(dispatchable_heads(f).len(), 2);
    // the appends past the array part grow it through the helper, which
    // the trace survives
    assert_eq!(entries, 2);
}

#[test]
fn array_counts_stay_right_when_a_trace_fills_holes() {
    // slots filled out of order inside the array part: `#t` must see the
    // counts the inline stores keep
    let src = "
        return function()
            local lens = {}
            for r = 1, 40 do
                local t = {}
                for i = 1, 16 do t[i] = 0 end
                for i = 1, 16 do t[i] = nil end
                for i = 2, 16, 2 do t[i] = i end
                lens[#lens + 1] = #t
                for i = 1, 15, 2 do t[i] = i end
                lens[#lens + 1] = #t
            end
            return table.concat(lens, ' ', 1, 4), #lens
        end";
    run_both(src, &[], 3);
}

#[test]
fn string_sub_and_a_short_string_compare_stay_in_the_trace() {
    let src = "
        return function(s)
            local c = 0
            for p = 1, #s do
                if string.sub(s, p, p) == ':' then c = p end
            end
            return c, string.sub(s, c + 1)
        end";
    let arg = |vm: &mut Vm| Value::Str(vm.heap.intern(b"session:user:7:counter:123"));
    let mut vm = trace_vm();
    let f = function(&mut vm, src);
    let s = arg(&mut vm);
    for _ in 0..3 {
        call(&mut vm, f, &[s]);
    }
    let before = vm.trace_dispatched_count();
    let r = call(&mut vm, f, &[s]);
    assert_eq!(show(&r), "23,123");
    assert_eq!(dispatchable_heads(f).len(), 1);
    // the colon branch is taken inside the trace: one entry for the loop
    assert_eq!(vm.trace_dispatched_count() - before, 1);
}

#[test]
fn a_comparison_s_other_branch_runs_inside_the_trace() {
    // whichever way the recorded pass went, the other way rejoins in the
    // trace instead of leaving it
    let src = "
        return function(n)
            local last3, last5, big = 0, 0, 0
            for i = 1, n do
                if i % 3 == 0 then last3 = i end
                if i % 5 ~= 0 then last5 = i end
                if i < 7 then big = i end
            end
            return last3, last5, big
        end";
    let (_vm, f, entries) = run_both(src, &[Value::Int(1000)], 3);
    assert_eq!(dispatchable_heads(f).len(), 1);
    assert_eq!(entries, 1);
}

#[test]
fn string_keys_and_string_constants_stay_in_the_trace() {
    let src = "
        local t = {}
        local keys = {}
        for i = 1, 100 do t['k' .. i] = i keys[i] = 'k' .. i end
        return function()
            local s, last = 0, ''
            for i = 1, #keys do
                local v = t[keys[i]]
                if v then s = s + v end
                last = 'done'
            end
            return s, last
        end";
    let (_vm, f, entries) = run_both(src, &[], 3);
    assert_eq!(dispatchable_heads(f).len(), 1);
    assert_eq!(entries, 1);
}

/// A generic-for trace that cannot loop inside itself returns to its head
/// after each iteration; the loop variables it read for the next one must
/// go back to the interpreter. They were left as they were, so the body
/// ran again on the old value and every element the trace fetched was
/// skipped (a metatable built in a loop was missing metamethods).
#[test]
fn a_generic_for_trace_hands_back_the_next_loop_variables() {
    let src = r#"
        local mt = {}
        for _, e in ipairs({"add", "sub", "mul", "div", "mod", "pow"}) do
          local k = "__" .. e
          mt[k] = function() return e end
        end
        local ks = {}
        for k, f in pairs(mt) do ks[#ks + 1] = k .. "=" .. f() end
        table.sort(ks)
        return table.concat(ks, " ")"#;
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(false);
        vm.jit.trace_hot_threshold = 1;
        let r = vm.eval(src).expect("runs");
        assert_eq!(
            show(&r),
            "__add=add __div=div __mod=mod __mul=mul __pow=pow __sub=sub",
            "{v:?}"
        );
        assert!(vm.trace_dispatched_count() > 0, "{v:?}: no trace ran");
    }
}

/// An inline array or field read whose slot no longer holds what the
/// trace was recorded with (a key past the array part, a field moved by a
/// rehash) leaves the trace for the interpreter, which runs the read, and
/// the loop goes on in the trace.
#[test]
fn an_inline_read_that_misses_leaves_the_trace_and_comes_back() {
    let src = "
        return function()
            local t = {1, 2, 3, 4, 5, 6, 7, 8}
            local o = {a = 1}
            local s = 0
            for i = 1, 400 do
                local v = t[i % 10 + 1]
                if v then s = s + v end
                s = s + o.a
                if i == 200 then
                    for k = 1, 20 do o['k' .. k] = k end
                end
            end
            return s
        end";
    let (_vm, f, entries) = run_both(src, &[], 3);
    // the outer loop and the one that reshapes `o`
    assert_eq!(dispatchable_heads(f).len(), 2);
    // each miss is one more entry; the loop still runs in the trace
    assert!(entries > 2 && entries < 200, "{entries} entries");
}
