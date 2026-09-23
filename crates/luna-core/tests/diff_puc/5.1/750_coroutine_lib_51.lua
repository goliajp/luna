-- v3.1 iopkg: the 5.1 coroutine library (lbaselib) differs from lcorolib.
-- running() returns one value (nil on the main thread); a body must be a
-- Lua function; thread errors say "coroutine expected"; resume names the
-- refused status; isyieldable/close do not exist; a yield below pcall
-- crosses the C boundary, and the message never carries a position.
-- Position prefixes are reduced to "POS:" because chunknames differ.
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("running main", coroutine.running())
p("running co", coroutine.resume(coroutine.create(function() return select("#", coroutine.running()) end)))
p("has isyieldable", coroutine.isyieldable ~= nil, rawget(coroutine, "close") ~= nil)
p("create C", pcall(coroutine.create, string.len))
p("wrap table", pcall(coroutine.wrap, {}))
p("status number", pcall(coroutine.status, 1))
p("resume none", pcall(coroutine.resume))
p("resume self", coroutine.resume(coroutine.create(function() return coroutine.resume(coroutine.running()) end)))
local outer
outer = coroutine.create(function()
  return coroutine.resume(coroutine.create(function() return coroutine.resume(outer) end))
end)
p("resume normal", coroutine.resume(outer))
p("yield main", pcall(coroutine.yield))
p("yield in pcall", coroutine.resume(coroutine.create(function() return pcall(coroutine.yield, 1) end)))
local w
w = coroutine.wrap(function() return w() end)
p("wrap reentrant", pcall(function() return w() end))
