-- v3.1 iopkg: 5.5 lcorolib. A coroutine has no message handler, so an
-- error(nil) inside it reaches resume/wrap/close as "<no error object>";
-- close refuses the main thread as "cannot close main thread".
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("resume nil error", coroutine.resume(coroutine.create(function() error(nil) end)))
p("wrap nil error", pcall(coroutine.wrap(function() error() end)))
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function() error(nil) end})
  coroutine.yield()
end)
coroutine.resume(co)
p("close nil error", coroutine.close(co))
p("close main", pcall(coroutine.close, (coroutine.running())))
p("close none main", pcall(coroutine.close))
p("isyieldable number", pcall(coroutine.isyieldable, 3))
p("wrap number", pcall(coroutine.wrap, 3))
