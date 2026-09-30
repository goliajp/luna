-- Return0 / Return1 into Lua callers: every wanted-result count, returns
-- that must close upvalues, returns under pcall and coroutines, and the
-- return hook, all as before
local function show(...)
  local t = {}
  for i = 1, select("#", ...) do t[#t + 1] = tostring((select(i, ...))) end
  print(select("#", ...), table.concat(t, " "))
end
local function r0() return end
local function r1(x) return x end
local function r1k() return "k" end
show(r0())
show(r1(1))
show((r1(2)))
show(r1(3), r1(4))
show(r1k(), r0(), r1k())
local a, b, c = r1(5)
show(a, b, c)
local d = r0()
show(d)
local t = {r1(6), r1(7), r1(8)}
show(#t, t[1], t[2], t[3], select("#", r1(6), r0(), r1(8)))
-- a returning frame with a captured local closes its upvalue first
local function mk(v)
  local function get() return v end
  return get
end
local g1, g2 = mk(10), mk(20)
show(g1(), g2())
local function counter()
  local n = 0
  return function() n = n + 1; return n end
end
local ctr = counter()
ctr(); ctr()
show(ctr())
-- under pcall (a continuation below) and inside coroutines
show(pcall(r1, 9))
show(pcall(r0))
local co = coroutine.wrap(function(x) local y = r1(x) coroutine.yield(y) return r1(y + 1) end)
show(co(11), co())
-- deep recursion through Return1
local function sum(n) if n == 0 then return 0 end return n + sum(n - 1) end
show(sum(200))
-- the return hook still sees every return
local rets = 0
debug.sethook(function(e) if e == "return" then rets = rets + 1 end end, "r")
r0(); r1(1); r1k()
debug.sethook()
show(rets >= 3)
-- Return0 / Return1 from a frame holding a to-be-closed variable run
-- its __close before the caller sees the result
local log = {}
local function closer(name) return setmetatable({}, {__close = function() log[#log + 1] = name end}) end
local function f() local x <close> = closer("f") return 1 end
local function g() local x <close> = closer("g") return end
print(f(), g(), table.concat(log, ","))
