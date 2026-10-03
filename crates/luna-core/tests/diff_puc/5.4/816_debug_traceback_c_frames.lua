-- Tracebacks a message handler takes where an error is raised, C function
-- levels included, in each dialect's wording: library functions as C
-- levels ('error', 'gsub', 'sort'), every way a function gets its name,
-- tail calls, metamethods, and the levels a deep stack leaves out. Each
-- case is a chunk loaded under its own name, called by `xpcall` inside a
-- coroutine: the coroutine's stack holds only the case's levels, so line
-- numbers and level counts are the same under PUC and luna.
local load = loadstring or load
local function run(src)
  local f = assert(load(src, "=t"))
  local r = assert(load([[
local f = ...
return coroutine.wrap(function()
  local ok, tb = xpcall(f, debug.traceback)
  return tb
end)()]], "=r"))
  -- 5.2 names a C function by a walk of the global table whose order
  -- changes from run to run: 'error' on one run, '_G.error' on the next
  print((r(f):gsub("'_G%.", "'")))
end
run([[return string.gsub("a", "a", error)]])
run([[local function f() return string.gsub("a", "a", error) end
f()]])
run([[local t = {}
function t.field(x) if x == 1 then error("field") end return x end
function t:method(x) local r = t.field(x); return r end
gfun = function(x) local r = t:method(x); return r end
local function loc(x) local r = gfun(x); return r end
local up = function(x) local r = loc(x); return r end
local function viaup(x) local r = up(x); return r end
local function tail(x) return viaup(x) end
local r = tail(1)]])
run([[local m = {}
function m.f() error("in mod") end
package.loaded.tbmod = m
local function g() m.f() end
g()
package.loaded.tbmod = nil]])
run([[local function a() error("tail err") end
local function b() return a() end
local function c() return b() end
c()]])
run([[local o = setmetatable({}, {__add = function() error("add") end,
  __index = function(t, k) error("idx " .. k) end})
local function h() return o + 1 end
local _ = o.zz]])
run([[local o = setmetatable({}, {__add = function() error("add") end})
local function h() return o + 1 end
h()]])
run([[table.sort({3, 2, 1}, function(a, b) return a.x < b end)]])
run([[local function f() return ("x"):rep({}) end
f()]])
run([[local function chk(v) assert(v, "assert failed") end
chk(false)]])
run([[local function f() error("x", 2) end
local function g() f() end
g()]])
run([[local x
x()]])
run([[local function f(t) return t.x.y end
f({})]])
run([[local co = coroutine.wrap(function() local function inner() error("in coro") end inner() end)
co()]])
run([[local o = setmetatable({}, {__tostring = function(s) error("ts") end})
local s = tostring(o)]])
for _, n in ipairs({14, 15, 16, 17, 18, 19, 20, 40}) do
  run(string.format([[local function rec(n) if n == 0 then error("deep") end local r = rec(n - 1); return r end
rec(%d)]], n))
end
run([[local function r2(n) if n == 0 then error("c levels") end
  table.sort({2, 1, 3}, function(a, b) r2(n - 1) return a < b end) end
r2(12)]])
-- the message: a string or number gets the traceback appended, anything
-- else comes back untouched, and nil is a traceback without a message
print(debug.traceback("msg"):match("^msg\nstack traceback:\n") ~= nil)
print(debug.traceback(12):match("^12\nstack traceback:\n") ~= nil)
local t = {}
print(debug.traceback(t) == t, debug.traceback(true))
print(debug.traceback():match("^stack traceback:\n") ~= nil)
print(select(2, coroutine.wrap(function()
  return xpcall(function() error({}) end, debug.traceback)
end)()) ~= nil)
