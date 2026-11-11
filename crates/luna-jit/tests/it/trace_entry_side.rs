//! Entry side traces: a loop whose trace refuses the register types it is
//! reached with gets a trace of its own for them, attached to the first,
//! which the dispatcher enters once the first refuses; at most three, and
//! none from code the dispatcher could not enter. Results are the
//! interpreter's throughout.

use luna_core::__private::{ProtoTraceInternals, VmJitState};
use luna_jit::runtime::Value;
use luna_jit::runtime::function::Proto;
use luna_jit::runtime::heap::Gc;
use luna_jit::version::LuaVersion;

pub(super) const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// What a run gives and counts.
#[derive(Debug, Default)]
pub(super) struct Ran {
    pub(super) result: String,
    pub(super) dispatched: u64,
    pub(super) compiled: u64,
    pub(super) closed: u64,
    pub(super) side: u64,
    /// The entry side traces the loaded chunk's functions hold.
    pub(super) entry_sides: usize,
    /// What the fast loop notes, per trace head of the chunk's functions.
    pub(super) refusals: Vec<u32>,
}

/// The entry side traces of the traces of `p` and the functions in it.
pub(super) fn entry_sides(p: Gc<Proto>) -> usize {
    let own: usize = p.traces().iter().map(|t| t.entry_side_traces().len()).sum();
    own + p.protos.iter().map(|q| entry_sides(*q)).sum::<usize>()
}

fn refusals(p: Gc<Proto>, out: &mut Vec<u32>) {
    out.extend(
        p.trace_head_refusals()
            .into_iter()
            .filter(|&r| r != u32::MAX),
    );
    for q in p.protos.iter() {
        refusals(*q, out);
    }
}

/// The text a run returns, the first value.
pub(super) fn text(r: Result<Vec<Value>, String>) -> String {
    match r {
        Ok(v) => match v.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {e}"),
    }
}

/// `src` run with the trace JIT on or off.
pub(super) fn run(v: LuaVersion, src: &str, jit: bool) -> Ran {
    luna_core::__private::set_len_search_seed(Some(1));
    let mut vm = luna_jit::vm::Vm::new_minimal_with_hash_seed(v, 1);
    luna_jit::install_default_jit(&mut vm);
    vm.open_all_libs().expect("the libraries open");
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let cl = vm.load(src.as_bytes(), b"=es").expect("load");
    // its functions are walked after the run: kept from the collector
    vm.pin_host(Value::Closure(cl));
    let r = vm
        .call_value(Value::Closure(cl), &[])
        .map_err(|e| vm.error_text(&e));
    let c = vm.jit_counters_mut();
    let mut ran = Ran {
        result: text(r),
        dispatched: c.dispatched,
        compiled: c.compiled,
        closed: c.closed,
        side: c.side_trace_compiled,
        entry_sides: entry_sides(cl.proto),
        refusals: Vec::new(),
    };
    refusals(cl.proto, &mut ran.refusals);
    ran
}

/// `src` with the trace JIT on, its result checked against the
/// interpreter's.
pub(super) fn checked(v: LuaVersion, src: &str) -> Ran {
    let jit = run(v, src, true);
    assert_eq!(jit.result, run(v, src, false).result, "{v:?}: {src}");
    jit
}

/// A vararg function (left to the interpreter by the method JIT) whose
/// loop compares `x` with the index: its trace is specialised to `x`'s
/// type. `{phases}` calls it on; each call's count goes in `r`.
pub(super) fn program(phases: &str) -> String {
    format!(
        "local function f(x, n, ...) local c = select('#', ...) \
         for i = 1, n do if x == i then c = c + 1 end c = c + 1 end return c end \
         local r = {{}} \
         local function call(x, n) r[#r + 1] = f(x, n) end \
         {phases} \
         return table.concat(r, ' ')"
    )
}

#[test]
fn a_loop_refusing_a_type_gets_an_entry_side_trace_for_it() {
    let numbers = program("call(0, 2000)");
    // a number entered: from 5.3 on a float is refused, before that (all
    // numbers one type) a string or a table
    // (a float compared with the integer index makes a trace that does not
    // compile: the float goes in an accumulator, as in the cliff benchmark)
    let cliff = "local function f(x, n, ...) local s = x + select('#', ...) \
                 for i = 1, n do s = s + i end return s end \
                 local r = {f(0, 2000), f(0.5, 20000), f(3, 2000)} \
                 return table.concat(r, ' ')"
        .to_string();
    let later = [cliff, program("call(0, 2000) call({}, 20000)")];
    let early = [
        program("call(0, 2000) call('s', 20000)"),
        program("call(0, 2000) call({}, 20000)"),
    ];
    for v in VERSIONS {
        let base = checked(v, &numbers);
        let srcs = if v >= LuaVersion::Lua53 {
            &later
        } else {
            &early
        };
        for src in srcs {
            let es = checked(v, src);
            assert_eq!(es.entry_sides, 1, "{v:?}: {src}: {es:?}");
            assert!(es.dispatched > base.dispatched, "{v:?}: {es:?} {base:?}");
        }
    }
}

#[test]
fn types_changing_back_and_forth_enter_both_and_record_one() {
    let src = program("call(0, 2000) for k = 1, 6 do call('s', 3000) call(k, 3000) end");
    for v in VERSIONS {
        let r = checked(v, &src);
        assert_eq!(r.entry_sides, 1, "{v:?}: {r:?}");
    }
}

#[test]
fn a_root_holds_three_and_the_fourth_type_is_passed_by_in_the_fast_loop() {
    let src = program(
        "call(0, 2000) call('s', 4000) call({}, 4000) call(print, 4000) \
         call(true, 20000)",
    );
    for v in VERSIONS {
        let r = checked(v, &src);
        assert_eq!(r.entry_sides, 3, "{v:?}: {r:?}");
        assert!(
            !r.refusals.is_empty(),
            "{v:?}: the fourth type was not noted: {r:?}"
        );
    }
}

#[test]
fn an_entry_side_trace_that_cannot_be_dispatched_is_not_attached_and_its_budget_runs_out() {
    // for a string the loop reads a native function: a trace the
    // dispatcher may not enter
    let src = "local function f(x, n, ...) local c = 0 \
               for i = 1, n do if x == 's' then local g = string.len end c = c + 1 end \
               return c end \
               local r = {f(0, 2000)} \
               for k = 1, 40 do r[#r + 1] = f('s', 2000) end \
               r[#r + 1] = f(1, 2000) \
               return table.concat(r, ' ')";
    for v in VERSIONS {
        let r = checked(v, src);
        assert_eq!(r.entry_sides, 0, "{v:?}: {r:?}");
        // three tries take the budget: compiles stop growing with the calls
        assert!(r.compiled <= 1 + 3, "{v:?}: {r:?}");
    }
}

#[test]
fn branches_of_the_root_and_of_its_entry_side_trace_each_get_side_traces() {
    let src = "local function f(x, n, ...) local c = x * 0 \
               for i = 1, n do if i % 3 == 0 then c = c + 2 else c = c + x end end \
               return c end \
               local r = {f(1, 3000)} r[#r + 1] = f(0.5, 30000) \
               return table.concat(r, ' ')";
    for v in &VERSIONS[2..] {
        let r = checked(*v, src);
        assert_eq!(r.entry_sides, 1, "{v:?}: {r:?}");
        assert!(
            r.side >= 2,
            "{v:?}: the two traces' exits did not both get one: {r:?}"
        );
    }
}

#[test]
fn what_the_head_does_first_and_calls_inlined_only_for_the_new_type_come_out_the_same() {
    // the loop's first instruction writes a table; for a string only, the
    // loop calls a function, inlined into the entry side trace alone
    let src = "local t = {} local function h(i) return i % 7 end \
               local function f(x, n, ...) local c = 0 \
               for i = 1, n do t[i % 5 + 1] = x if x == 's' then c = c + h(i) end c = c + 1 end \
               return c end \
               local r = {f(0, 2000), f('s', 20000), f(2, 2000)} \
               r[#r + 1] = t[1] r[#r + 1] = t[5] \
               return table.concat(r, ' ')";
    for v in VERSIONS {
        checked(v, src);
    }
}

#[test]
fn short_alternating_calls_errors_and_coroutines_give_the_interpreters_results() {
    let alternating =
        program("call(0, 2000) for k = 1, 400 do call(k % 2 == 0 and k or 's', 10) end");
    let errors = "local function f(x, n, ...) local c = 0 \
                  for i = 1, n do if x == 's' and i == n then error('boom at ' .. i) end c = c + 1 end \
                  return c end \
                  local r = {f(0, 2000)} \
                  for k = 1, 3 do local ok, e = pcall(f, 's', 3000) r[#r + 1] = tostring(e) end \
                  return table.concat(r, ' ')";
    let coros = "local function f(x, n, ...) local c = 0 \
                 for i = 1, n do if x == i then c = c + 1 end c = c + 1 end \
                 coroutine.yield(c) return c end \
                 local r = {} \
                 for _, x in ipairs({0, 's', 0, 's'}) do \
                   local co = coroutine.wrap(f) r[#r + 1] = co(x, 3000) r[#r + 1] = co() end \
                 return table.concat(r, ' ')";
    for v in VERSIONS {
        checked(v, &alternating);
        checked(v, errors);
        checked(v, coros);
    }
}

#[test]
fn a_head_whose_trace_cannot_be_dispatched_is_not_recorded_again_and_again() {
    // the loop calls a closure: too short a trace to dispatch
    let src = "local function g(i) return i end \
               local function f(n) local t = {} for i = 1, n do t[i % 8 + 1] = g(i) end return #t end \
               local r = {} for k = 1, 20 do r[#r + 1] = f(5000) end \
               return table.concat(r, ' ')";
    for v in VERSIONS {
        let r = checked(v, src);
        assert!(r.closed <= 4, "{v:?}: recorded again and again: {r:?}");
    }
}

#[test]
fn a_loop_whose_head_has_a_side_trace_of_an_earlier_loop_still_gets_its_own() {
    // the first loop's exit resumes at the second loop's head
    let src = "local function f(n) local s, k = 0, 0 \
               for i = 1, n do s = s + i % 7 end \
               while k < n do k = k + 1 s = s + k % 3 end \
               return s end \
               local r = {} for c = 1, 200 do r[#r + 1] = f(300) end \
               return r[1] .. ' ' .. r[200]";
    for v in VERSIONS {
        checked(v, src);
    }
}
