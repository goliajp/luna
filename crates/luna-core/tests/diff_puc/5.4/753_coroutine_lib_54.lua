-- v3.1 iopkg: 5.4 lcorolib. isyieldable accepts an optional thread (checked
-- with luaL_argexpected), close requires its thread argument, and a wrap
-- error from a string is prefixed with the caller's position.
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("isyieldable co", coroutine.isyieldable(coroutine.create(string.len)))
p("isyieldable number", pcall(coroutine.isyieldable, 3))
p("isyieldable nil", pcall(coroutine.isyieldable, nil))
p("close none", pcall(coroutine.close))
p("wrap table", pcall(coroutine.wrap, {}))
local w
w = coroutine.wrap(function() return w() end)
p("wrap reentrant", pcall(function() return w() end))
p("yield main", pcall(coroutine.yield))
