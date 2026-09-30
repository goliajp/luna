-- table reads through every read opcode: a raw hit is the answer even
-- when __index exists, a miss follows the __index chain (tables and
-- functions), and keys of every kind find the same slots
local log = {}
local function show(...)
  local t = {}
  for i = 1, select("#", ...) do t[#t + 1] = tostring((select(i, ...))) end
  print(table.concat(t, " "))
end
local base = {inherited = "base", shared = "base"}
local mid = setmetatable({shared = "mid", flag = false}, {__index = base})
local obj = setmetatable({own = 1, [1] = "one", [2.0] = "two"}, {__index = mid})
show(obj.own, obj.shared, obj.inherited, obj.flag, obj.missing)
show(obj[1], obj[2], obj[1.0], obj[3], obj["own"])
local fn = setmetatable({}, {__index = function(t, k) log[#log + 1] = tostring(k); return "fn:" .. tostring(k) end})
show(fn.a, fn[1], fn[2.5], fn[true])
show(table.concat(log, ","))
-- long string keys: equal content, different objects
local long1 = string.rep("k", 50)
local long2 = string.rep("kk", 25)
local lt = {[long1] = "long"}
show(lt[long2], lt[long1 .. ""], rawequal(long1, long2))
-- short strings built at run time are the same key as literals
local s = "ab" .. "c"
local st = {abc = 3}
show(st[s], st.abc, st["a" .. "bc"])
-- method calls: own method, inherited method, method via function __index
local Class = {}
Class.__index = Class
function Class.new(v) return setmetatable({v = v}, Class) end
function Class:get() return self.v end
local o = Class.new(7)
o.get2 = function(self) return self.v * 2 end
show(o:get(), o:get2(), ("x"):rep(3), ("abc"):upper())
-- array part, hash part, holes, negative and zero keys
local a = {10, 20, 30, nil, 50, [0] = "zero", [-1] = "neg"}
show(a[1], a[3], a[4], a[5], a[0], a[-1], a[6])
-- globals through _ENV's metatable
setmetatable(_G, {__index = function(_, k) return "global:" .. k end})
show(undefined_name, print ~= nil)
setmetatable(_G, nil)
-- chains that end in a raw false, and rawget agreeing with the fast path
local chain = setmetatable({}, {__index = setmetatable({}, {__index = {deep = false}})})
show(chain.deep, rawget(chain, "deep"), rawget(obj, "own"))
