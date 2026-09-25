-- v3.1 iopkg: 5.3 lcorolib. Thread arguments say "thread expected" without
-- the offending type, isyieldable takes no argument (a thread passed in is
-- ignored), close does not exist, and a wrapped call of a running coroutine
-- with an empty frame still reports "dead".
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("has close", rawget(coroutine, "close") ~= nil)
p("status number", pcall(coroutine.status, 1))
p("resume table", pcall(coroutine.resume, {}))
p("isyieldable arg", coroutine.isyieldable(coroutine.create(string.len)), pcall(coroutine.isyieldable, 3))
p("wrap none", pcall(coroutine.wrap))
local w
w = coroutine.wrap(function() return w() end)
p("wrap reentrant", pcall(function() return w() end))
p("yield main", pcall(coroutine.yield))
