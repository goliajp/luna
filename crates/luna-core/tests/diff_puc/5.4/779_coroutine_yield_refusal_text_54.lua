-- v3.1 iopkg follow-up: a refused yield raises lua_yield's own message,
-- which carries no position (the running call is the yield itself). 5.1
-- says "attempt to yield across metamethod/C-call boundary" for every case;
-- 5.2+ say "attempt to yield across a C-call boundary", or "from outside a
-- coroutine" on the main thread. A wrapped coroutine's error is prefixed
-- with the position of the wrapped function's caller: a string always, and
-- before 5.3 a number too.
local function clean(s) return (tostring(s):gsub("'_G%.", "'"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("sort comparator", coroutine.resume(coroutine.create(function()
  table.sort({3, 2, 1}, function(a, b) coroutine.yield() return a < b end)
end)))
p("gsub callback", coroutine.resume(coroutine.create(function()
  return string.gsub("a", "a", function() coroutine.yield() end)
end)))
p("main thread", pcall(coroutine.yield))
p("raw message", coroutine.resume(coroutine.create(function()
  local ok, e = pcall(table.sort, {2, 1}, function() coroutine.yield() end)
  return e
end)))
local w = coroutine.wrap(function()
  error(42, 0)
end)
p("wrap number", pcall(function()
  return w()
end))
w = coroutine.wrap(function()
  error("s", 0)
end)
p("wrap string", pcall(function()
  return w()
end))
w = coroutine.wrap(function()
  error({}, 0)
end)
p("wrap table", pcall(function()
  local ok, e = pcall(w)
  return type(e)
end))
