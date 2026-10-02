//! Coroutines and yields across call boundaries.

use super::*;

#[test]
fn coroutine_basics() {
    // two-way value passing through resume/yield
    check_int(
        "local co = coroutine.create(function(a) local b = coroutine.yield(a+1) return b*10 end) \
         local _, x = coroutine.resume(co, 4) \
         local _, y = coroutine.resume(co, 7) \
         return x*100 + y",
        570, // x=5, y=70
    );
    // status transitions
    check_str(
        "local co = coroutine.create(function() coroutine.yield() end) \
         local a = coroutine.status(co) coroutine.resume(co) \
         local b = coroutine.status(co) coroutine.resume(co) \
         local c = coroutine.status(co) return a..','..b..','..c",
        b"suspended,suspended,dead",
    );
    // wrap: generator captured upvalues resolve to the creating thread's stack
    check_int(
        "local function gen(t) for _,v in ipairs(t) do coroutine.yield(v) end end \
         local data = {3, 4, 5} \
         local sum = 0 \
         for v in coroutine.wrap(function() gen(data) end) do sum = sum + v end \
         return sum",
        12,
    );
    // resuming a dead coroutine fails softly
    check_bool(
        "local co = coroutine.create(function() end) coroutine.resume(co) \
         local ok = coroutine.resume(co) return ok",
        false,
    );
    // an error inside the coroutine surfaces as (false, msg)
    check_str(
        "local co = coroutine.create(function() error('boom') end) \
         local ok, msg = coroutine.resume(co) \
         return tostring(ok)..':'..(msg:match('boom') or '?')",
        b"false:boom",
    );
    // yield outside a coroutine is an error
    check_error(
        "coroutine.yield(1)",
        "attempt to yield from outside a coroutine",
    );
    // running() reports the main thread
    check_bool("local _, m = coroutine.running() return m", true);

    // yield across a transparent native frame: a chunk run by the
    // native `dofile` yields, then resume re-enters and continues the suspended
    // chunk frame to completion, its final return flowing back through dofile to
    // the resumer. Exercises call_value not truncating the suspended stack +
    // coro_continue restoring the frame's full register window.
    check_int(
        "local p = os.tmpname() \
         local w = assert(io.open(p, 'w')) \
         w:write('local x, z = coroutine.yield(10)\\n') \
         w:write('local y = coroutine.yield(20)\\n') \
         w:write('return x + y * z\\n') \
         w:close() \
         local co = coroutine.wrap(dofile) \
         local a = co(p) \
         local b = co(100, 101) \
         local c = co(7) \
         os.remove(p) \
         return a * 1000000 + b * 1000 + c",
        10_020_807,
    );

    // coroutine.close reports the error a coroutine died with, once: a thread
    // killed by error(100) closes to (false, 100), then to (true, nil).
    check_str(
        "local co = coroutine.create(error) \
         local _, m1 = coroutine.resume(co, 100) \
         local s2, m2 = coroutine.close(co) \
         local s3, m3 = coroutine.close(co) \
         return m1 .. ',' .. tostring(s2) .. ',' .. m2 .. ',' .. tostring(s3) .. ',' .. tostring(m3)",
        b"100,false,100,true,nil",
    );

    // coroutine.close runs the suspended coroutine's pending <close> handlers
    // (with nil error) and returns true; a handler that raises makes close
    // report (false, err).
    check_str(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local trace = {} \
         local co = coroutine.create(function () \
           local a <close> = c(function (_, e) trace[#trace+1] = 'a:'..tostring(e) end) \
           coroutine.yield() \
         end) \
         coroutine.resume(co) \
         local ok = coroutine.close(co) \
         return tostring(ok) .. ',' .. table.concat(trace, ',') .. ',' .. coroutine.status(co)",
        b"true,a:nil,dead",
    );
    check_str(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.create(function () \
           local a <close> = c(function () error('boom') end) \
           coroutine.yield() \
         end) \
         coroutine.resume(co) \
         local ok, err = coroutine.close(co) \
         return tostring(ok) .. ',' .. (err:match('boom') or '?')",
        b"false,boom",
    );

    // yield across `pcall`: a Lua function protected by pcall
    // yields, then resumes to completion; pcall wraps the final return as
    // (true, ...). Exercises the continuation frame surviving a yield.
    check_str(
        "local function f() coroutine.yield(10); return 20 end \
         local co = coroutine.create(function() return pcall(f) end) \
         local _, y1 = coroutine.resume(co) \
         local ok, st, v = coroutine.resume(co) \
         return y1 .. ',' .. tostring(ok) .. ',' .. tostring(st) .. ',' .. v",
        b"10,true,true,20",
    );
    // an error in the protected function after a yield is caught as (false, msg)
    check_str(
        "local function f() coroutine.yield(); error('boom') end \
         local co = coroutine.create(function() return pcall(f) end) \
         coroutine.resume(co) \
         local _, ok, msg = coroutine.resume(co) \
         return tostring(ok) .. ',' .. (msg:match('boom') or '?')",
        b"false,boom",
    );
    // coroutine.create(pcall): pcall itself is the body; the protected function
    // is passed on the first resume and may yield through pcall.
    check_str(
        "local co = coroutine.create(pcall) \
         local _, y = coroutine.resume(co, function() return coroutine.yield(5) + 1 end) \
         local ok, st, v = coroutine.resume(co, 100) \
         return y .. ',' .. tostring(ok) .. ',' .. tostring(st) .. ',' .. v",
        b"5,true,true,101",
    );
    // xpcall across a yield: the message handler runs on the post-yield error
    check_str(
        "local function f() coroutine.yield(); error('boom') end \
         local co = coroutine.create(function() return xpcall(f, function(m) return 'H:'..m end) end) \
         coroutine.resume(co) \
         local _, ok, msg = coroutine.resume(co) \
         return tostring(ok) .. ',' .. tostring(msg:match('^H:') ~= nil) .. ',' .. (msg:match('boom') or '?')",
        b"false,true,boom",
    );
}

#[test]
fn pcall_continuation_no_yield() {
    // success: results wrapped as (true, ...)
    check_str(
        "local ok, a, b = pcall(function() return 1, 2 end) \
         return tostring(ok) .. ',' .. a .. ',' .. b",
        b"true,1,2",
    );
    // error caught as (false, msg)
    check_str(
        "local ok, msg = pcall(function() error('x') end) \
         return tostring(ok) .. ',' .. (msg:match('x') or '?')",
        b"false,x",
    );
    // protected native function (no Lua frame pushed for it)
    check_str(
        "local ok, n = pcall(math.type, 1.0) return tostring(ok) .. ',' .. n",
        b"true,float",
    );
    // the caller's full register window survives an error caught by pcall
    // (the unwind truncates into it, then it is reinstated)
    check_int(
        "local function ce(f) local s = pcall(f); assert(not s) end \
         ce(function() error('e') end) \
         local a,b,c,d,e,f,g,h = 1,2,3,4,5,6,7,8 \
         return a+b+c+d+e+f+g+h",
        36,
    );
    // a <close> handler runs during the unwind, then pcall still catches
    check_str(
        "local function c(fn) return setmetatable({}, {__close = fn}) end \
         local seen \
         local ok, msg = pcall(function() \
           local x <close> = c(function(_, e) seen = e end) \
           error('boom') \
         end) \
         return tostring(ok) .. ',' .. (msg:match('boom') or '?') .. ',' .. (seen:match('boom') or '?')",
        b"false,boom,boom",
    );
    // the protected-call C-stack bound: self-recursive pcall terminates at a
    // bounded depth (~MAX_C_DEPTH) rather than running away to the Lua-stack
    // limit or overflowing the native stack
    check_bool(
        "local n = 0 \
         local function rec() n = n + 1; return pcall(rec) end \
         pcall(rec) \
         return n > 100 and n < 2000",
        true,
    );
    // xpcall: message handler transforms the error
    check_str(
        "local ok, m = xpcall(function() error('boom') end, function(msg) return 'H:'..msg end) \
         return tostring(ok) .. ',' .. tostring(m:match('^H:') ~= nil) .. ',' .. (m:match('boom') or '?')",
        b"false,true,boom",
    );
}

#[test]
fn yield_across_c_boundary() {
    // a thread is non-yieldable inside an unprotected C call (gsub replacement)
    check_str(
        "local r \
         coroutine.wrap(function() \
           string.gsub('a', 'a', function() r = coroutine.isyieldable() end) \
         end)() \
         return tostring(r)",
        b"false",
    );
    // yielding across that boundary errors rather than panicking; the sort is
    // itself a C call, so the yield surfaces as a protected-call failure
    check_str(
        "local co = coroutine.wrap(function() \
           local ok, msg = pcall(table.sort, {3, 1, 2}, coroutine.yield) \
           return tostring(ok) .. ',' .. (msg:match('C%-call boundary') or '?') \
         end) \
         return co()",
        b"false,C-call boundary",
    );
    // a coroutine closing itself (PUC 5.5): the to-be-closed handler runs and
    // the thread dies cleanly — resume yields (true) with no extra values, and
    // code after the close is unreachable
    check_str(
        "local c = function(f) return setmetatable({}, {__close = f}) end \
         local X = 'no' \
         local co = coroutine.create(function() \
           local v <close> = c(function() X = 'closed' end) \
           string.gsub('a', 'a', function() \
             coroutine.close() \
             X = 'unreachable' \
           end) \
         end) \
         local st, msg = coroutine.resume(co) \
         return tostring(st) .. ',' .. tostring(msg) .. ',' .. X .. ',' .. coroutine.status(co)",
        b"true,nil,closed,dead",
    );
    // if the self-close handler raises, the error becomes the coroutine's death
    // error (propagated past the protecting pcalls, not caught by them)
    check_str(
        "local c = function(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.create(function() \
           local v <close> = c(function() error('boom') end) \
           string.gsub('a', 'a', function() \
             assert(pcall(pcall, function() coroutine.close() end)) \
           end) \
         end) \
         local st, msg = coroutine.resume(co) \
         return tostring(st) .. ',' .. (msg:match('boom') or '?')",
        b"false,boom",
    );
    // a generic-for iterator, by contrast, is yieldable (it is called by the VM,
    // not via an unprotected C call): yielding through it suspends normally
    check_str(
        "local function iter(_, i) return coroutine.yield(i) end \
         local co = coroutine.wrap(function() \
           for i in iter, nil, 1 do end \
         end) \
         return tostring(co()) .. ',' .. tostring(co(7))",
        b"1,7",
    );
    // a chain of coroutines whose __close handlers each close the previous one
    // bottoms out as a (recoverable) "C stack overflow", not a panic
    check_str(
        "local coro = false \
         for i = 1, 1000 do \
           local previous = coro \
           coro = coroutine.create(function() \
             local cc <close> = setmetatable({}, {__close = function() \
               if previous then assert(coroutine.close(previous)) end \
             end}) \
             coroutine.yield() \
           end) \
           assert(coroutine.resume(coro)) \
         end \
         local st, msg = coroutine.close(coro) \
         return tostring(st) .. ',' .. (msg:match('C stack overflow') or '?')",
        b"false,C stack overflow",
    );
}

#[test]
fn coroutine_resume_refuses_too_many_results() {
    // PUC `auxresume` (lcorolib.c) calls `lua_checkstack(L, nres + 1)` on
    // the parent thread before transferring the coroutine's return values.
    // A coroutine that produces near-`LUAI_MAXSTACK` values into its own
    // stack still cannot deliver them when the caller's stack has no room.
    // 5.3 coroutine.lua :530's `for j in {lim-10, lim-5, …}` series pins
    // this — every j from `lim - 10` upward must fail.
    check_str(
        "local lim = 1000000 \
         local out = {} \
         for _, j in ipairs{lim - 10, lim - 5, lim - 1, lim, lim + 1} do \
             local co = coroutine.create(function () \
                 local t = {} \
                 for i = 1, j do t[i] = i end \
                 return table.unpack(t) \
             end) \
             local r = coroutine.resume(co) \
             out[#out + 1] = tostring(r) \
         end \
         return table.concat(out, ',')",
        b"false,false,false,false,false",
    );
}

#[test]
fn yield_inside_pairs_metamethod() {
    // nextvar.lua:953 — a coroutine.yield() inside a __pairs metamethod called
    // by pairs() must suspend cleanly (pairs drives __pairs as a continuation).
    check_int(
        "local t = setmetatable({10, 20, 30}, {__pairs = function (t) \
           local inc = coroutine.yield() \
           return function (t, i) if i > 1 then return i - inc, t[i - inc] end end, t, #t + 1 \
         end}) \
         local sum = 0 \
         local co = coroutine.wrap(function () for _, p in pairs(t) do sum = sum + p end end) \
         co(); co(1) \
         return sum",
        60,
    );
}
