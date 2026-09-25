-- v3.1 iopkg: 5.2 lcorolib. create/wrap use luaL_checktype (the offending
-- type is named), thread arguments say "coroutine expected", isyieldable
-- and close do not exist yet, a wrapped call of a running coroutine with an
-- empty frame reports "dead", and a yield on the main thread has no position.
-- '_G.' is stripped (5.2 may name C-called functions through _G).
local function clean(s) return (tostring(s):gsub("'_G%.", "'"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("running main", select("#", coroutine.running()), select(2, coroutine.running()))
p("has isyieldable", coroutine.isyieldable ~= nil, rawget(coroutine, "close") ~= nil)
p("create C", pcall(coroutine.create, string.len) and "ok")
p("wrap table", pcall(coroutine.wrap, {}))
p("create none", pcall(coroutine.create))
p("status number", pcall(coroutine.status, 1))
p("resume self", coroutine.resume(coroutine.create(function() return coroutine.resume(coroutine.running()) end)))
p("yield main", pcall(coroutine.yield))
local w
w = coroutine.wrap(function() return w() end)
p("wrap reentrant", pcall(function() return w() end))
w = coroutine.wrap(function() error("in body") end)
p("wrap error position", pcall(function() return w() end))
