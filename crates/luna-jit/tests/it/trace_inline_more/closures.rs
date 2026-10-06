//! Closures made in a frame of a function the trace inlined: they take the
//! upvalues of that frame's own closure, and those of its locals as they
//! are when the function returns.

use super::*;
use luna_jit::jit::trace::INLINE_CLOSURE;

fn closures(vm: &Vm) -> u64 {
    vm.trace_inline_kind_dispatched_count(INLINE_CLOSURE)
}

#[test]
fn callee_returns_a_closure_over_its_parameter() {
    let src = "local function adder(x) return function(y) return x + y end end
        return function()
          local s = 0
          for i = 1, 500 do s = s + adder(i)(1) end
          return s
        end";
    assert_every_tier(src, 4, "closure in an inlined frame", closures);
}

#[test]
fn closure_captures_a_local_changed_after_it_is_made() {
    let src = "local function make(x)
          local n = x
          local f = function() return n end
          n = n * 2
          return f
        end
        return function()
          local s = 0
          for i = 1, 500 do s = s + make(i)() end
          return s
        end";
    assert_every_tier(src, 4, "closure in an inlined frame", closures);
}

#[test]
fn closure_takes_an_upvalue_of_the_callee() {
    let src = "local base = 10
        local function make(x) return function() return base + x end end
        return function()
          local s = 0
          for i = 1, 500 do
            if i == 300 then base = 20 end
            s = s + make(i)()
          end
          return s
        end";
    assert_every_tier(src, 4, "closure in an inlined frame", closures);
}

#[test]
fn closures_kept_in_a_table_see_their_own_values() {
    let src = "local function make(x) local c = x return function() c = c + 1 return c end end
        return function()
          local fs = {}
          for i = 1, 300 do fs[i] = make(i) end
          collectgarbage()
          local s = 0
          for i = 1, 300, 7 do s = s + fs[i]() + fs[i]() end
          return s
        end";
    agree(src, 3, closures);
}

#[test]
fn closure_made_on_an_exit_path() {
    let src = "local function make(x)
          local f = function() return x end
          if x % 5 == 0 then return nil end
          return f
        end
        return function()
          local s, n = 0, 0
          for i = 1, 500 do
            local f = make(i)
            if f then s = s + f() else n = n + 1 end
          end
          return s, n
        end";
    agree(src, 4, closures);
}

#[test]
fn dialects_agree() {
    let src = "local function make(x) local y = x * 2 return function() return x + y end end
        return function()
          local s = 0
          for i = 1, 400 do s = s + make(i)() end
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
