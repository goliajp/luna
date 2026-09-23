-- v3.1 iopkg: an error that nothing inside a coroutine catches kills it
-- without unwinding its stack (lua_resume), so its pending to-be-closed
-- variables run only when it is closed: by coroutine.close, or by
-- coroutine.wrap, which closes the dead coroutine before re-raising. A
-- pcall inside the coroutine still closes on the way out.
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: "):gsub("0x%x+", "ADDR")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, table.concat(t, " ")) end
local function mk(tag) return setmetatable({}, {__close = function(_, e) print("close", tag, clean(e)) end}) end
local co = coroutine.create(function() local x <close> = mk("resume"); error("boom") end)
p("resume", coroutine.resume(co))
p("status", coroutine.status(co))
p("close", coroutine.close(co))
local w = coroutine.wrap(function() local x <close> = mk("wrap"); error("boom2") end)
p("wrap", pcall(w))
w = coroutine.wrap(function() local x <close> = setmetatable({}, {__close = function() error("in close") end}); error("orig") end)
p("wrap replaced", pcall(w))
co = coroutine.create(function()
  local ok = pcall(function() local y <close> = mk("inner pcall"); error("caught") end)
  local x <close> = mk("outer")
  error("escapes")
end)
p("mixed", coroutine.resume(co))
p("mixed close", coroutine.close(co))
