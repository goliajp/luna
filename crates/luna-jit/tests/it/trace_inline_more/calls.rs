//! Calls into vararg functions, calls that want several values or all of
//! them, and calls that pass a variable number of arguments, inlined into
//! the trace: the callee's frame is laid out as `push_frame` lays it out,
//! in the trace's registers and in the frames an exit rebuilds.

use super::*;
use luna_jit::jit::trace::{INLINE_MULTI_RESULTS, INLINE_VAR_ARGS, INLINE_VARARG_CALLEE};

fn kind(k: u8) -> impl Fn(&Vm) -> u64 {
    move |vm| vm.trace_inline_kind_dispatched_count(k)
}

#[test]
fn vararg_callee_reads_its_arguments() {
    let src = "local function f(...) local a, b = ... return a + b end
        return function()
          local s = 0
          for i = 1, 500 do s = s + f(i, i * 2) end
          return s
        end";
    assert_every_tier(src, 4, "vararg callee", kind(INLINE_VARARG_CALLEE));
}

#[test]
fn vararg_callee_with_fixed_parameters_and_extras() {
    let src = "local function f(x, ...) local y, z = ... return x * y + (z or 0) end
        return function()
          local s = 0
          for i = 1, 500 do s = s + f(i, 3) + f(i, 2, i) end
          return s
        end";
    assert_every_tier(src, 4, "vararg callee", kind(INLINE_VARARG_CALLEE));
}

#[test]
fn vararg_callee_given_fewer_arguments_than_parameters() {
    let src = "local function f(a, b, ...) local c = ... return (b or 1) + (c or 2) + a end
        return function()
          local s = 0
          for i = 1, 500 do s = s + f(i) end
          return s
        end";
    assert_every_tier(src, 4, "vararg callee", kind(INLINE_VARARG_CALLEE));
}

#[test]
fn vararg_callee_builds_a_table_of_its_arguments() {
    let src = "local function f(...) local t = {...} return #t + t[1] end
        return function()
          local s = 0
          for i = 1, 500 do s = s + f(i, 2, 3) end
          return s
        end";
    agree(src, 4, kind(INLINE_VARARG_CALLEE));
}

#[test]
fn call_wants_two_values() {
    let src = "local function f(x) return x, x * 2 end
        return function()
          local s = 0
          for i = 1, 500 do local a, b = f(i); s = s + a + b end
          return s
        end";
    assert_every_tier(src, 4, "multiple results", kind(INLINE_MULTI_RESULTS));
}

#[test]
fn call_wants_more_values_than_returned() {
    let src = "local function f(x) return x end
        return function()
          local s, n = 0, 0
          for i = 1, 500 do
            local a, b, c = f(i)
            s = s + a
            if b == nil and c == nil then n = n + 1 end
          end
          return s, n
        end";
    assert_every_tier(src, 4, "multiple results", kind(INLINE_MULTI_RESULTS));
}

#[test]
fn all_values_passed_on_to_another_call() {
    let src = "local function f(x) return x, x + 1, x + 2 end
        local function g(a, b, c) return a * b - c end
        return function()
          local s = 0
          for i = 1, 500 do s = s + g(f(i)) end
          return s
        end";
    for kinds in [INLINE_VAR_ARGS, INLINE_MULTI_RESULTS] {
        assert_every_tier(src, 4, "variable argument count", kind(kinds));
    }
}

#[test]
fn all_values_passed_to_a_vararg_callee() {
    let src = "local function f(x) return x, -x end
        local function g(...) local a, b = ... return a - b end
        return function()
          local s = 0
          for i = 1, 500 do s = s + g(f(i)) end
          return s
        end";
    assert_every_tier(src, 4, "variable argument count", kind(INLINE_VAR_ARGS));
}

#[test]
fn all_values_handed_to_a_native_function() {
    // the trace ends at the call to `select`, which reads the stack top
    // the inlined call left
    let src = "local function f(x) if x % 2 == 0 then return x, x end return x end
        return function()
          local s = 0
          for i = 1, 500 do s = s + select('#', f(i)) end
          return s
        end";
    agree(src, 4, kind(INLINE_MULTI_RESULTS));
}

#[test]
fn all_values_into_a_table_constructor() {
    let src = "local function f(x) return x, x + 1 end
        return function()
          local s = 0
          for i = 1, 500 do local t = {f(i)}; s = s + #t + t[2] end
          return s
        end";
    agree(src, 4, kind(INLINE_MULTI_RESULTS));
}

#[test]
fn exit_inside_a_vararg_callee() {
    let src = "local function f(...)
          local a, b = ...
          if a % 7 == 0 then return select('#', ...) end
          return a + b
        end
        return function()
          local s = 0
          for i = 1, 700 do s = s + f(i, 1) end
          return s
        end";
    assert_every_tier(src, 4, "vararg callee", kind(INLINE_VARARG_CALLEE));
}

#[test]
fn error_inside_a_vararg_callee_has_the_frames() {
    let src = "local function f(...)
          local a, t = ...
          return a + t.v
        end
        return function()
          local t = {v = 1}
          local ok, err = pcall(function()
            local s = 0
            for i = 1, 300 do
              if i == 250 then t = nil end
              s = s + f(i, t)
            end
            return s
          end)
          return ok, err
        end";
    agree(src, 3, kind(INLINE_VARARG_CALLEE));
}

#[test]
fn dialects_agree() {
    let src = "local function f(x, ...) local y = ... return x + (y or 0), x end
        local function g(...) local a, b = ... return a * b end
        return function()
          local s = 0
          for i = 1, 400 do s = s + g(f(i, 2)) end
          return s
        end";
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        agree_in(v, src, 3, |_| ());
    }
}

/// An exit inside a function inlined two calls deep rebuilds both frames;
/// the middle one resumes after its own call, here to add 1.
#[test]
fn exit_two_calls_deep_resumes_the_middle_frame() {
    let src = "local function h(y)
          if y > 1500 then return y end
          return y * 3
        end
        local function f(x)
          if x % 5 == 0 then return h(x) + 1 end
          return x
        end
        return function()
          local s = 0
          for i = 1, 2000 do s = s + f(i) end
          return s
        end";
    assert_every_tier(src, 4, "trace entries", |vm| vm.trace_dispatched_count());
}

/// A callee the trace cannot hold (it reads a global the trace cannot
/// type): the recording is compiled up to the call instead, and the trace
/// still runs the loop's other work (enough of it that a trace ending at a
/// call is worth entering).
#[test]
fn callee_the_trace_cannot_hold_ends_the_trace_at_the_call() {
    let pad = "a = a + i b = b ~ a\n".repeat(24);
    let src = format!(
        "local function va(...) return select('#', ...), ... end
        return function()
          local s, a, b = 0, 0, 0
          for i = 1, 300 do
            {pad}
            local n, x = va(i)
            s = s + n + x
          end
          return s, a, b
        end"
    );
    let counts = agree(&src, 3, |vm| {
        (vm.trace_dispatched_count(), vm.trace_inline_cut_count())
    });
    for (k, (n, cut)) in counts.into_iter().enumerate() {
        assert!(
            n > 0 && cut > 0,
            "{}: dispatched {n}, cut {cut}",
            TIERS[k].0
        );
    }
}
