-- v3.1 debug slice: tracebacks of other threads — suspended (level 0 is
-- the yield), dead by an error (the stack it died with, from any level),
-- and a coroutine waiting on one it resumed (inside coroutine.resume).
local function norm(tb)
  tb = tb:gsub("[%w_%.%-]+:%d+:", "SRC:")
  tb = tb:gsub("<[%w_%.%-]+:%d+>", "<SRC>")
  return tb
end
local function f(n) if n > 0 then coroutine.yield(); f(n - 1) else error("x") end end
local co = coroutine.create(function(x) f(x) end)
coroutine.resume(co, 1)
print(norm(debug.traceback(co)))
print(norm(debug.traceback(co, "msg", 1)))
coroutine.resume(co)
print(norm(debug.traceback(co)))
print(norm(debug.traceback(co, nil, 1)))
print(norm(debug.traceback(co, nil, 2)))
local outer
outer = coroutine.create(function()
  coroutine.resume(coroutine.create(function()
    print(norm(debug.traceback(outer, "normal")))
  end))
end)
coroutine.resume(outer)
