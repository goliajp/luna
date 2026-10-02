//! To-be-closed variables and their close handlers.

use super::*;

#[test]
fn to_be_closed() {
    // closed on normal block exit, in reverse order
    check_str(
        "local log = '' local function tracker(n) return setmetatable({}, \
           {__close = function() log = log .. n end}) end \
         do local a <close> = tracker('a') local b <close> = tracker('b') end \
         return log",
        b"ba",
    );
    // closed on error, handler sees the error object
    check_str(
        "local seen local t = setmetatable({}, {__close = function(_, e) seen = e end}) \
         local ok, err = pcall(function() local x <close> = t error('boom', 0) end) \
         return seen",
        b"boom",
    );
    // closed when a loop body iterates
    check_int(
        "local n = 0 local mt = {__close = function() n = n + 1 end} \
         for i = 1, 3 do local x <close> = setmetatable({}, mt) end return n",
        3,
    );
    // nil/false are silently accepted, others must be closable
    check_int(
        "do local x <close> = nil local y <close> = false end return 1",
        1,
    );
    check_error("local x <close> = 42", "non-closable value");
    check_compile_error(
        "local a <close>, b <close> = nil, nil",
        "multiple to-be-closed",
    );
    // close vars are read-only
    check_compile_error(
        "local x <close> = nil x = 1",
        "attempt to assign to const variable 'x'",
    );
    // normal close passes only the object (1 arg)
    check_int(
        "local n local mt = {__close = function(...) n = select('#', ...) end} \
         do local y <close> = setmetatable({}, mt) end return n",
        1,
    );
    // error close passes the object and the error object (2 args)
    check_int(
        "local n local mt = {__close = function(...) n = select('#', ...) end} \
         pcall(function() local y <close> = setmetatable({}, mt) error('e', 0) end) return n",
        2,
    );
    // an error in a __close handler chains to the next handler's error object
    check_bool(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local ok, msg = pcall(function() \
           local x <close> = c(function(_, m) assert(m:find('@y')) error('@x') end) \
           local y <close> = c(function(_, m) assert(m == nil) error('@y') end) \
         end) \
         return msg:find('@x') ~= nil",
        true,
    );
    // a __close handler that triggers GC doesn't lose the pending error object
    check_bool(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local ok, msg = pcall(function() \
           local x <close> = c(function(_, m) assert(m:find('@y')) error('@x') end) \
           local g <close> = c(function() collectgarbage() end) \
           local y <close> = c(function(_, m) assert(m == nil) error('@y') end) \
         end) \
         return msg:find('@x') ~= nil",
        true,
    );
    // `return f()` inside tbc scope is not a tail call (returns all results)
    check_int(
        "local function multi() return 1, 2, 3 end \
         local function bar() local _ <close> = setmetatable({}, {__close=function() end}) \
           do return multi() end end \
         local a, b, c = bar() return a + b + c",
        6,
    );
}

#[test]
fn close_handler_debug_parent_is_enclosing_function() {
    // locals.lua:288 — a __close handler on a normal exit runs within the
    // closing function's activation, so debug.getinfo(2) names that function
    // (PUC luaF_close; the handler is not a synthetic C boundary).
    check_str(
        "local captured \
         local function foo() \
           local _ <close> = setmetatable({}, {__close = function () \
             captured = debug.getinfo(2).name \
           end}) \
           return 1 \
         end \
         foo() \
         return captured",
        b"foo",
    );
}

#[test]
fn xpcall_traceback_sees_close_handler_frame() {
    // locals.lua:544 — debug.traceback called as xpcall msgh after a __close
    // handler raised must name the handler frame "metamethod 'close'" (PUC
    // luaG_errormsg runs msgh at the error point with stack intact). luna
    // snapshots the traceback at unwind entry so the catcher's msgh sees it.
    check_int(
        "local _, msg = xpcall(function () \
           local _ <close> = setmetatable({}, {__close = function () error('boom') end}) \
         end, debug.traceback) \
         return string.find(msg, \"in metamethod 'close'\") and 1 or 0",
        1,
    );
}

#[test]
fn non_closable_value_at_tbc_names_variable() {
    // locals.lua:554 — `local x <close> = {}` (no __close mm) errors with
    // "variable 'x' got a non-closable value (a table value)" (PUC
    // checkclosemth pulls the local name from the running frame's locvars).
    check_int(
        "local ok, msg = pcall(function () local x <close> = {} end) \
         return (not ok) and string.find(msg, \"variable 'x' got a non%-closable value\") and 1 or 0",
        1,
    );
}

#[test]
fn close_handler_removed_metamethod_errors() {
    // locals.lua:562 — __close was present at OP_TBC but cleared before close
    // time. luna's close_slots no longer silently skips: it raises
    // "attempt to call a <T> value (metamethod 'close')" (PUC
    // prepclosingmethod treats it as a non-callable target at close time).
    check_int(
        "local ok, msg = pcall(function () \
           local x <close> = setmetatable({}, {__close = print}) \
           getmetatable(x).__close = nil \
         end) \
         return (not ok) and string.find(msg, \"metamethod 'close'\") and 1 or 0",
        1,
    );
}

#[test]
fn stack_overflow_recovery_runs_close_in_errorh() {
    // locals.lua:659 — xpcall(overflow, errorh) where errorh sets up a
    // `<close>` local. The unwind restored the stack to the error-point
    // length (near MAX_LUA_STACK), so the next call_value_impl picked a
    // func_slot beyond the limit and re-overflowed. unwind now clamps the
    // restore to the catcher's caller window + MIN_STACK reserve.
    check_int(
        "local function overflow (n) overflow(n + 1) end \
         local function errorh (m) \
           local x <close> = setmetatable({}, {__close = function (o) o[1] = 42 end}) \
           return x \
         end \
         local _, obj = xpcall(overflow, errorh) \
         return obj[1]",
        42,
    );
}

#[test]
fn yieldable_close_at_block_exit() {
    // locals.lua:858 — a `do ... end` block's `<close>` may yield through its
    // __close handler; the block's OP_Close drives close handlers via the
    // interpreter loop, so a resume continues the close cleanly. Trace records
    // the order of body / close-enter / close-exit so we can detect a yield
    // that did not actually suspend.
    check_str(
        "local trace = {} \
         local function f2c(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.wrap(function () \
           do \
             local z <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'z1'; coroutine.yield('z'); trace[#trace + 1] = 'z2' \
             end) \
           end \
           trace[#trace + 1] = 'after' \
         end) \
         assert(co() == 'z') \
         co() \
         return table.concat(trace, ',')",
        b"z1,z2,after",
    );
}

#[test]
fn yieldable_close_at_function_return() {
    // locals.lua:874 — OP_Return's __close chain yields, then resumes to
    // deliver the original results to the caller (here, the `return x, X, 23`
    // pattern from locals.lua:277). The handler's `stack(10)` recursion is
    // the existing repro that shook out a self.top vs. abs_a + nret
    // off-by-one (post-close handler clobbered results).
    check_int(
        "local function f2c(f) return setmetatable({}, {__close = f}) end \
         local trace = {} \
         local co = coroutine.wrap(function () \
           local function foo (x) \
             local _ <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'y1'; coroutine.yield('y'); trace[#trace + 1] = 'y2' \
             end) \
             return x, 23 \
           end \
           local a, b = foo(1.5) \
           assert(a == 1.5 and b == 23) \
           trace[#trace + 1] = 'done' \
         end) \
         assert(co() == 'y') \
         co() \
         assert(trace[1] == 'y1' and trace[2] == 'y2' and trace[3] == 'done') \
         return 1",
        1,
    );
}

#[test]
fn yieldable_close_during_error_unwind() {
    // locals.lua :625..:1015 — `__close` handlers run during error unwind may
    // also yield; the Lua frame is popped before the close so `getinfo(2)`
    // names the C boundary (pcall), and `AfterClose::ResumeUnwind` defers
    // truncate + re-raise until every handler in the chain has run.
    check_str(
        "local trace = {} \
         local function f2c(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.wrap(function () \
           local function foo () \
             local x <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'x1'; coroutine.yield('x'); trace[#trace + 1] = 'x2' \
             end) \
             local y <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'y1'; coroutine.yield('y'); trace[#trace + 1] = 'y2' \
             end) \
             error('boom') \
           end \
           local ok, msg = pcall(foo) \
           assert(not ok and msg:find('boom')) \
           trace[#trace + 1] = 'done' \
         end) \
         assert(co() == 'y') \
         assert(co() == 'x') \
         co() \
         return table.concat(trace, ',')",
        b"y1,y2,x1,x2,done",
    );
}

#[test]
fn close_handler_debug_parent_on_error_unwind_is_c_boundary() {
    // locals.lua :480 — during error unwind, `__close` handlers run after
    // their host Lua frame has been popped, so `getinfo(2).name` is the
    // outer caller (`pcall`), not the aborting function. Regressed once
    // when we deferred the frame pop until the close drained.
    check_str(
        "local function f2c(f) return setmetatable({}, {__close = f}) end \
         local got = '?' \
         local function foo () \
           local _ <close> = f2c(function (_, msg) \
             got = debug.getinfo(2).name or 'nil' \
           end) \
           error('boom') \
         end \
         pcall(foo) \
         return got",
        b"pcall",
    );
}

#[test]
fn goto_out_of_nested_for_closes_iterator_close_values() {
    // locals.lua :1219 — a `goto` leaving a generic-for loop must close the
    // iterator's implicit closing value (the 4th control slot). The body
    // block's reg_floor lands at `base + 4`, so the trampoline OP_Close must
    // target `base` (not `base + 4`) for the closing slot at `base + 3`.
    check_int(
        "local numopen = 0 \
         local function f2c(f) return setmetatable({}, {__close = f}) end \
         local function open (n) \
           numopen = numopen + 1 \
           return function () n = n - 1; if n > 0 then return n end end, \
                  nil, nil, \
                  f2c(function () numopen = numopen - 1 end) \
         end \
         local s = 0 \
         for i in open(3) do \
           for j in open(3) do \
             if i + j < 3 then goto endloop end \
             s = s + i \
           end \
         end \
         ::endloop:: \
         assert(numopen == 0, 'open iterators leaked: ' .. numopen) \
         return s",
        5,
    );
}

#[test]
fn close_handler_debug_parent_is_lua_on_normal_close() {
    // PUC luaF_close: a normal (non-error) close handler runs *within* the
    // closing function's activation; getinfo(2).what must be "Lua", not "C".
    // Regressed once when begin_close passed `!error_close` for `from_c`.
    check_str(
        "local function f2c(f) return setmetatable({}, {__close = f}) end \
         local what = '?' \
         local function foo () \
           local _ <close> = f2c(function (_, msg) \
             what = debug.getinfo(2).what or 'nil' \
           end) \
           return 1 \
         end \
         foo() \
         return what",
        b"Lua",
    );
}
