-- what Lua code sees when a host native panics: `boom` panics with a
-- string, `boom42` with a non-string payload, `boomif(x)` returns 1 unless
-- `x` is true, `boomiter` is a generic-for iterator that panics at 1500
local boom, boom42, boomif, boomiter = ...
local out = {}
local function log(...)
  local t = {}
  for i = 1, select('#', ...) do t[#t + 1] = tostring((select(i, ...))) end
  out[#out + 1] = table.concat(t, " ")
end
local function show(tag, ...) log(tag, select('#', ...), ...) end
local loadstr = loadstring or load
local v54 = _VERSION >= "Lua 5.4"

show("pcall", pcall(boom))
show("pcall-nonstr", pcall(boom42))
show("pcall-args", pcall(boom, 1, 2, 3))
show("xpcall-tb", xpcall(boom, debug.traceback))
show("xpcall-info", xpcall(boom, function(m)
  local t = {}
  for l = 1, 6 do
    local i = debug.getinfo(l, "Sln")
    if not i then break end
    t[#t + 1] = i.what .. ":" .. tostring(i.name) .. ":" .. tostring(i.currentline)
  end
  return m .. " | " .. table.concat(t, ",")
end))

local function tail(x) return boom(x) end
local function plain(x) local r = boom(x) return r end
local function deeper(x) local r = plain(x) return r end
show("lua-tail", pcall(tail, 1))
show("lua-plain", pcall(plain, 1))
show("lua-deeper-tb", xpcall(deeper, debug.traceback, 1))
show("nested", pcall(function()
  local ok, e = pcall(boom)
  error("outer:" .. tostring(ok) .. ":" .. e, 0)
end))
show("nested-xpcall", xpcall(function()
  return select(2, xpcall(boom, function(m) return "inner<" .. m .. ">" end))
end, function(m) return "outer<" .. m .. ">" end))
show("pcall-chain", pcall(pcall, pcall, boom))
show("error-in-handler", xpcall(boom, function(m) boom() end))

local co = coroutine.create(function() coroutine.yield(1) plain(2) end)
show("co-1", coroutine.resume(co))
show("co-2", coroutine.resume(co))
log("co-status", coroutine.status(co))
log("co-tb", debug.traceback(co))
show("co-wrap", pcall(coroutine.wrap(function() boom() end)))
-- 5.1 accepts only a Lua function here
show("co-direct", pcall(function() return coroutine.resume(coroutine.create(boom)) end))
show("co-in-pcall", coroutine.resume(coroutine.create(function() return pcall(boom) end)))

local mt = {
  __index = boom, __newindex = boom, __add = boom, __concat = boom,
  __unm = boom, __call = boom, __lt = boom, __le = boom, __eq = boom, __len = boom,
}
local a, b = setmetatable({}, mt), setmetatable({}, mt)
show("mm-index", pcall(function() return a.x end))
show("mm-newindex", pcall(function() a.x = 1 end))
show("mm-add", pcall(function() return a + 1 end))
show("mm-concat", pcall(function() return a .. "x" end))
show("mm-unm", pcall(function() return -a end))
show("mm-call", pcall(function() return a(1) end))
show("mm-lt", pcall(function() return a < b end))
show("mm-le", pcall(function() return a <= b end))
show("mm-eq", pcall(function() return a == b end))
show("mm-len", pcall(function() return #a end))
show("mm-tostring", pcall(tostring, setmetatable({}, {__tostring = boom})))
show("mm-index-tb", xpcall(function() return a.x end, debug.traceback))

show("lib-sort", pcall(table.sort, {3, 2, 1}, boom))
show("lib-gsub", pcall(string.gsub, "abc", "%w", boom))
show("lib-load-reader", pcall(load, boom))
show("lib-select", pcall(select, 2, pcall(boom)))

debug.sethook(function() end, "", 1000)
show("hooked", pcall(plain, 1))
show("hooked-tb", xpcall(deeper, debug.traceback, 1))
debug.sethook()
local calls = {}
debug.sethook(function(ev) calls[#calls + 1] = ev end, "cr")
pcall(boom)
debug.sethook()
log("hook-events", #calls, table.concat(calls, ","))

if v54 then
  local f = loadstr([[
    local boom, log = ...
    local x <close> = setmetatable({}, {__close = function(o, e) log("close-saw", e) end})
    boom()
  ]])
  show("tbc", pcall(f, boom, log))
end

-- the hot loops take what they use as arguments, which keeps them
-- traceable: a native held in an upvalue stops a trace
local c = {0, 0, 0, 0}
show("loop", pcall(function(c, f)
  for i = 1, 2000 do c[1] = c[1] + f(i == 1500) end
end, c, boomif))
log("loop-n", c[1])
show("tfor", pcall(function(c, it)
  for k in it, nil, 0 do c[2] = c[2] + k end
end, c, boomiter))
log("tfor-s", c[2])
local ix = setmetatable({}, {__index = function(t, k) return boomif(k == 1500) end})
show("loop-mm", pcall(function(c, t)
  for i = 1, 2000 do c[3] = c[3] + t[i] end
end, c, ix))
log("loop-mm-m", c[3])
-- a trace that stops at a call is run only when the code before the call
-- is long enough, whatever the dialect's loops compile to
local long = loadstr("local c, f = ... for i = 1, 2000 do local x = i "
  .. string.rep("x = x + 1 ", 45) .. "c[4] = c[4] + f(i == 1500) + x - x end")
show("loop-long", pcall(long, c, boomif))
log("loop-long-n", c[4])

-- after the panics the stack reads as before (the recursion depth to a
-- stack overflow is not stable from run to run, so it is not compared)
local function tb() return select(2, xpcall(deeper, debug.traceback, 1)) end
local tbs = {}
for i = 1, 2 do
  if i == 2 then
    for _ = 1, 20 do pcall(boom) pcall(plain, 1) pcall(function() return a + 1 end) end
  end
  tbs[i] = tb()
end
log("tb-same", tbs[1] == tbs[2])
log("traceback-here", debug.traceback("here"))
return table.concat(out, "\n")
