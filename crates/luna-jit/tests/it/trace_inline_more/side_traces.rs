//! Side traces started at a hot exit inside a function the parent trace
//! inlined: recorded in that function's frame, cached on its prototype,
//! run by the dispatcher from the parent's exit on the registers of the
//! frame that exit rebuilt.

use super::*;

fn inlined_runs(vm: &Vm) -> u64 {
    vm.trace_side_trace_inlined_run_count()
}

#[test]
fn branch_taken_inside_a_callee() {
    let src = "local function f(x, t)
          if x % 4 == 0 then
            t[#t + 1] = x
            return x * 2
          end
          return x + 1
        end
        return function()
          local t, s = {}, 0
          for i = 1, 2000 do s = s + f(i, t) end
          return s, #t
        end";
    assert_every_tier(src, 4, "side trace runs inside the callee", inlined_runs);
}

#[test]
fn branch_taken_two_calls_deep() {
    let src = "local function g(x) if x % 3 == 0 then return -x end return x end
        local function f(x) return g(x) + 1 end
        return function()
          local s = 0
          for i = 1, 2000 do s = s + f(i) end
          return s
        end";
    assert_every_tier(src, 4, "side trace runs two calls deep", inlined_runs);
}

#[test]
fn side_trace_inlines_a_call_and_leaves_inside_it() {
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
    assert_every_tier(src, 4, "side trace runs inside the callee", inlined_runs);
}

#[test]
fn method_with_a_branch_on_the_receiver() {
    let src = "local cls = {}; cls.__index = cls
        function cls:bump(i)
          if i % 4 == 0 then self.a = self.a + i else self.b = self.b + 1 end
          return self.a
        end
        return function()
          local o = setmetatable({a = 0, b = 0}, cls)
          local last = 0
          for i = 1, 3000 do last = o:bump(i) end
          return last, o.b
        end";
    assert_every_tier(src, 4, "side trace runs inside the method", inlined_runs);
}

#[test]
fn side_trace_creates_tables_and_strings() {
    let src = "local function f(x, out)
          if x % 6 == 0 then
            out[#out + 1] = {x, 'k' .. x}
            return 1
          end
          return 0
        end
        return function()
          local out, n = {}, 0
          for i = 1, 1200 do n = n + f(i, out) end
          collectgarbage()
          return n, #out, out[3][2], out[#out][1]
        end";
    agree(src, 4, inlined_runs);
}

#[test]
fn error_raised_in_a_side_trace_inside_a_callee() {
    let src = "local function f(x, t)
          if x % 4 == 0 then return t.v + x end
          return x
        end
        return function()
          local s, t = 0, {v = 1}
          for i = 1, 600 do
            if i == 500 then t = {} end
            s = s + f(i, t)
          end
          return s
        end";
    agree(src, 2, inlined_runs);
}

#[test]
fn self_recursive_exits_still_agree() {
    let src = "local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
        return function() return fib(20) end";
    agree(src, 3, inlined_runs);
}
