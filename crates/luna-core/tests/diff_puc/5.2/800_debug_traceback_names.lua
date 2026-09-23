-- v3.1 debug slice: luaL_traceback's per-version naming of each level
-- (global / local / method / field / upvalue, C functions by their
-- loaded-module name, tail calls). Source names and lines differ between
-- the harnesses, so they are masked, and the listing stops at the main
-- chunk (below it PUC has lua.c's own C function).
local function norm(tb)
  tb = tb:gsub("\n[^\n]*in main chunk.*$", "\n\t<main>")
  tb = tb:gsub("[%w_%.%-]+:%d+:", "SRC:")
  tb = tb:gsub("<[%w_%.%-]+:%d+>", "<SRC>")
  return tb
end
local t = {}
function t.field() return debug.traceback("field") end
function t:method() return debug.traceback("method") end
gfun = function() return debug.traceback("global") end
local function loc() return debug.traceback("local") end
local up = function() return debug.traceback("upvalue") end
local function tail() return up() end
print(norm(t.field()))
print(norm(t:method()))
print(norm(gfun()))
print(norm((function() local x = loc(); return x end)()))
print(norm((function() local x = up(); return x end)()))
print(norm((function() local x = tail(); return x end)()))
print(norm(select(2, pcall(debug.traceback, "via pcall"))))
local s
table.sort({2, 1}, function(a, b) s = s or debug.traceback("sort"); return a < b end)
print(norm(s))
print(norm(tostring(setmetatable({}, {__tostring = function() return debug.traceback("tostring") end}))))
print(norm(setmetatable({}, {__index = function() return debug.traceback("index") end}).x))
