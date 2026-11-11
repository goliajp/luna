//! Entry side traces taken up from other Vms, at a function's entry, from
//! finalizers, moved to the optimizing tier, and their failures counted
//! against their root only.

use luna_core::__private::ProtoTraceInternals;
use luna_jit::Engine;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

use super::trace_entry_side::{VERSIONS, checked, entry_sides, text};

/// The cliff benchmark's loop: an accumulator entered as an integer, then
/// as a float (from 5.3 on, another type).
const CLIFF: &str = "local function f(x, n, ...) local s = x + select('#', ...) \
                     for i = 1, n do s = s + i end return s end \
                     return function(x, n) return f(x, n) end";

/// The same with a branch, whose exits get side traces.
const BRANCHY: &str = "local function f(x, n, ...) local s = x + select('#', ...) \
                       for i = 1, n do if i % 3 == 0 then s = s + 2 else s = s + i end end \
                       return s end \
                       return function(x, n) return f(x, n) end";

fn engine_vm(engine: &Engine, v: LuaVersion) -> Vm {
    let mut vm = engine.new_vm(v);
    vm.set_jit_enabled(false);
    vm
}

/// What calls of the function `src` returns made in `vm`.
#[derive(Debug)]
struct Calls {
    results: Vec<String>,
    compiled: u64,
    adopted: u64,
    entry_sides: usize,
}

fn calls(vm: &mut Vm, src: &str, args: &[(f64, i64)]) -> Calls {
    let (compiled, adopted) = (vm.trace_compiled_count(), vm.trace_adopted_count());
    let main = vm.load(src.as_bytes(), b"=es").expect("load");
    // its functions are walked after the run: kept from the collector
    vm.pin_host(Value::Closure(main));
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    vm.pin_host(Value::Closure(f));
    let results = args
        .iter()
        .map(|&(x, n)| {
            let x = if x.fract() == 0.0 {
                Value::Int(x as i64)
            } else {
                Value::Float(x)
            };
            let r = vm.call_value(Value::Closure(f), &[x, Value::Int(n)]);
            text(r.map_err(|e| vm.error_text(&e)))
        })
        .collect();
    Calls {
        results,
        compiled: vm.trace_compiled_count() - compiled,
        adopted: vm.trace_adopted_count() - adopted,
        entry_sides: entry_sides(main.proto),
    }
}

fn interp(v: LuaVersion, src: &str, args: &[(f64, i64)]) -> Vec<String> {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    calls(&mut vm, src, args).results
}

#[test]
fn a_vm_takes_up_entry_side_traces_another_compiled() {
    let ints_then_floats = [(0.0, 2000), (0.5, 20000), (0.5, 20000)];
    let floats = [(0.5, 20000), (0.5, 20000)];
    for v in &VERSIONS[2..] {
        for src in [CLIFF, BRANCHY] {
            let engine = Engine::new();
            let a = calls(&mut engine_vm(&engine, *v), src, &ints_then_floats);
            assert_eq!(a.results, interp(*v, src, &ints_then_floats), "{v:?}");
            assert_eq!(a.entry_sides, 1, "{v:?}: {a:?}");
            // floats only: the float trace is this Vm's root
            let b = calls(&mut engine_vm(&engine, *v), src, &floats);
            assert_eq!(b.results, interp(*v, src, &floats), "{v:?}");
            assert_eq!((b.compiled, b.entry_sides), (0, 0), "{v:?}: {b:?}");
            assert!(b.adopted > 0, "{v:?}: {b:?}");
            // integers then floats: the float trace comes as an entry side
            // trace of the integer one, with the side traces it has
            let c = calls(&mut engine_vm(&engine, *v), src, &ints_then_floats);
            assert_eq!(c.results, a.results, "{v:?}");
            assert_eq!((c.compiled, c.entry_sides), (0, 1), "{v:?}: {c:?}");
        }
    }
}

#[test]
fn a_shared_trace_that_cannot_be_an_entry_side_trace_is_taken_up_a_few_times_at_most() {
    // for a float the loop reads a native function: a trace no dispatcher
    // may enter, published all the same
    let src = "local function f(x, n, ...) local s = x + select('#', ...) \
               for i = 1, n do if s ~= s // 1 then local g = string.len end s = s + i end \
               return s end \
               return function(x, n) return f(x, n) end";
    let args: Vec<(f64, i64)> = std::iter::once((0.0, 2000))
        .chain(std::iter::repeat_n((0.5, 3000), 30))
        .collect();
    for v in &VERSIONS[2..] {
        let engine = Engine::new();
        let a = calls(&mut engine_vm(&engine, *v), src, &args);
        assert_eq!(a.results, interp(*v, src, &args), "{v:?}");
        let c = calls(&mut engine_vm(&engine, *v), src, &args);
        assert_eq!(c.results, a.results, "{v:?}");
        assert_eq!(c.entry_sides, 0, "{v:?}: {c:?}");
        // the budget takes three refusals
        assert!(c.adopted <= 1 + 3, "{v:?}: {c:?}");
    }
}

#[test]
fn a_function_entered_with_other_types_gets_an_entry_side_trace_at_its_entry() {
    // fib's trace is recorded from its calls; its entry then refuses floats
    let src = "local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end \
               local r = {fib(18)} \
               r[#r + 1] = fib(1.5) \
               local once = {} for k = 1, 10 do once[k] = fib(k) end \
               r[#r + 1] = fib(18.0) r[#r + 1] = fib(17.0) \
               return table.concat(r, ' ')";
    for v in &VERSIONS[2..] {
        let r = checked(*v, src);
        assert!(r.entry_sides >= 1, "{v:?}: {r:?}");
    }
    // one odd call alone records none
    let once = "local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end \
                local r = {fib(18)} r[#r + 1] = fib(1.5) return table.concat(r, ' ')";
    for v in &VERSIONS[2..] {
        let r = checked(*v, once);
        assert_eq!(r.entry_sides, 0, "{v:?}: {r:?}");
    }
}

#[test]
fn an_entry_side_trace_recorded_from_a_finalizer_while_a_trace_runs() {
    // tables collected while the loop's trace runs call `f` from their
    // finalizer, with a type its root refuses
    let src = "local function f(x, n, ...) local s = x + select('#', ...) \
               for i = 1, n do s = s + i end return s end \
               local out = 0 \
               local mt = {__gc = function() out = out + f(0.5, 300) end} \
               local total = f(0, 2000) \
               for i = 1, 30000 do setmetatable({}, mt) total = total + i % 3 end \
               collectgarbage() \
               return tostring(total) .. ' ' .. tostring(out > 0)";
    for v in &VERSIONS[2..] {
        checked(*v, src);
    }
}

#[test]
fn entry_side_traces_move_to_the_optimizing_tier_on_their_own_count() {
    let src = "local function f(x, n, ...) local s = x + select('#', ...) \
               for i = 1, n do if i % 3 == 0 then s = s + 2 else s = s + i end end \
               return s end \
               local r = {f(0, 2000)} for k = 1, 20 do r[#r + 1] = f(0.5, 2000) end \
               return table.concat(r, ' ', 1, 3)";
    for v in &VERSIONS[2..] {
        for at in [1, 37] {
            let mut vm = luna_jit::new_with_jit(*v);
            vm.set_jit_enabled(false);
            vm.set_trace_tier_up_at(at);
            let cl = vm.load(src.as_bytes(), b"=es").expect("load");
            // its functions are walked after the run: kept from the collector
            vm.pin_host(Value::Closure(cl));
            let r = text(
                vm.call_value(Value::Closure(cl), &[])
                    .map_err(|e| vm.error_text(&e)),
            );
            assert_eq!(
                r,
                super::trace_entry_side::run(*v, src, false).result,
                "{v:?} {at}"
            );
            assert_eq!(entry_sides(cl.proto), 1, "{v:?} {at}");
            assert!(
                vm.trace_tiered_up_count() >= 2,
                "{v:?} {at}: the entry side trace stayed"
            );
        }
    }
}

#[test]
fn failed_entry_side_recordings_leave_the_root_head_and_the_fail_memory_alone() {
    // for a float the trace reads a native function (it cannot be
    // dispatched); the integer calls after it still enter the root, and
    // the same code loaded again records its root anew
    let src = "local function f(x, n, ...) local s = x + select('#', ...) \
               for i = 1, n do if s ~= s // 1 then local g = string.len end s = s + i end \
               return s end \
               local r = {f(0, 2000)} \
               for k = 1, 10 do r[#r + 1] = f(0.5, 3000) end \
               r[#r + 1] = f(1, 3000) \
               return table.concat(r, ' ', #r - 1)";
    for v in &VERSIONS[2..] {
        let mut vm = luna_jit::new_with_jit(*v);
        vm.set_jit_enabled(false);
        for load in 0..2 {
            // the same code loaded again takes up the traces it compiled
            let made = |vm: &Vm| vm.trace_compiled_count() + vm.trace_adopted_count();
            let before = made(&vm);
            let cl = vm.load(src.as_bytes(), b"=es").expect("load");
            // its functions are walked after the run: kept from the collector
            vm.pin_host(Value::Closure(cl));
            let r = text(
                vm.call_value(Value::Closure(cl), &[])
                    .map_err(|e| vm.error_text(&e)),
            );
            assert_eq!(
                r,
                super::trace_entry_side::run(*v, src, false).result,
                "{v:?}"
            );
            assert!(made(&vm) > before, "{v:?}: load {load} recorded no root");
            let dispatched = vm.trace_dispatched_count();
            assert!(dispatched > 0, "{v:?}");
        }
    }
}

/// The references to the traces of `src`'s function after its calls.
fn holders(v: LuaVersion, src: &str, args: &[(Value, i64)]) -> (Vec<usize>, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    let cl = vm.load(src.as_bytes(), b"=es").expect("load");
    // its functions are walked after the run: kept from the collector
    vm.pin_host(Value::Closure(cl));
    let f = match vm.call_value(Value::Closure(cl), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    vm.pin_host(Value::Closure(f));
    for (x, n) in args {
        vm.call_value(Value::Closure(f), &[*x, Value::Int(*n)])
            .expect("call");
    }
    let inner = cl.proto.protos[0];
    // roots and entry side traces; side traces are held by their parents
    let held = (inner.traces().iter())
        .filter(|t| !t.is_side_trace())
        .flat_map(|t| std::iter::once(t.clone()).chain(t.entry_side_traces()))
        .map(|t| luna_core::jit::send_compat::TArc::strong_count(&t))
        .collect();
    (held, vm.trace_side_trace_compiled_count())
}

#[test]
fn a_parent_is_not_held_by_its_side_traces() {
    // a side trace with a tier move keeps its recording, which names its
    // parent without holding it: a root and an entry side trace with side
    // traces are held as often as ones without
    let args = [(Value::Int(0), 3000), (Value::Float(0.5), 30000)];
    for v in &VERSIONS[2..] {
        let (plain, none) = holders(*v, CLIFF, &args);
        let (branchy, sides) = holders(*v, BRANCHY, &args);
        assert_eq!(none, 0, "{v:?}: a side trace in the plain loop");
        assert!(sides > 0, "{v:?}: no side trace");
        assert_eq!(plain, branchy, "{v:?}: a side trace holds its parent");
    }
}

#[test]
fn collection_settings_do_not_change_which_entry_side_traces_are_made() {
    let body = "local function f(x, n, ...) local s = x + select('#', ...) \
                for i = 1, n do s = s + i local t = {i} end return s end \
                local r = {f(0, 3000), f(0.5, 30000)} return table.concat(r, ' ')";
    for v in &VERSIONS[2..] {
        let mut made = Vec::new();
        for junk in [0, 2000, 20000] {
            let src = format!("local junk = {{}} for i = 1, {junk} do junk[i] = {{}} end {body}");
            let r = checked(*v, &src);
            made.push(r.entry_sides);
        }
        assert!(made.iter().all(|&n| n == made[0]), "{v:?}: {made:?}");
    }
}
