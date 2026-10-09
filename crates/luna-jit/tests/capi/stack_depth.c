/* How deep each kind of nesting gets before the error that ends it, and
   that error's text, for a script a C host runs with lua_pcall: the
   depth counts match PUC's exactly, as the host's stack layout and the
   limits it checks do. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char script[] =
  "-- how deep each kind of nesting gets before its error, and the error text;\n"
  "-- the Lua-stack overflows come last: after 5.1 catches one, its CallInfo\n"
  "-- array is halved by the collector, so only the first is deterministic\n"
  "local load = loadstring or load\n"
  "depth = 0\n"
  "local function norm(e) return (tostring(e):gsub(\"^[^\\n]-:%d+: \", \"@ \")) end\n"
  "local function run(name, f)\n"
  "  depth = 0\n"
  "  local ok, e = pcall(f)\n"
  "  print(name, depth, ok, norm(e))\n"
  "end\n"
  "local mt = {}\n"
  "local a, b = setmetatable({}, mt), setmetatable({}, mt)\n"
  "mt.__index = function(t, k) depth = depth + 1 return t[k] end\n"
  "mt.__newindex = function(t, k, v) depth = depth + 1 t[k] = v end\n"
  "mt.__eq = function(x, y) depth = depth + 1 return x == y end\n"
  "mt.__add = function(x, y) depth = depth + 1 return x + y end\n"
  "mt.__tostring = function(x) depth = depth + 1 return tostring(x) end\n"
  "mt.__call = function(s) depth = depth + 1 return (s()) end\n"
  "mt.__pairs = function(t) depth = depth + 1 for k in pairs(t) do end return next, t, nil end\n"
  "run(\"index\", function() return a.x end)\n"
  "run(\"newindex\", function() a.x = 1 end)\n"
  "run(\"eq\", function() return a == b end)\n"
  "run(\"add\", function() return a + 1 end)\n"
  "run(\"tostring\", function() return tostring(a) end)\n"
  "run(\"pairs\", function() for k in pairs(a) do end end)\n"
  "run(\"sort\", function() local function s() depth = depth + 1 table.sort({3, 2, 1}, function(x, y) s() return x < y end) end s() end)\n"
  "run(\"gsub\", function() local function g() depth = depth + 1 return (string.gsub(\"x\", \"x\", function() return g() end)) end return g() end)\n"
  "run(\"wrap\", function() local function w() depth = depth + 1 return coroutine.wrap(w)() end return w() end)\n"
  "run(\"resume\", function() local last local function r() depth = depth + 1 local ok, e = coroutine.resume(coroutine.create(r)) if not ok then last = e end end r() error(last, 0) end)\n"
  "run(\"pcall\", function() local last local function p() depth = depth + 1 local ok, e = pcall(p) if not ok then last = e end end p() error(last, 0) end)\n"
  "run(\"xpcall\", function() local last local function h(m) return m end local function p() depth = depth + 1 local ok, e = xpcall(p, h) if not ok then last = e end end p() error(last, 0) end)\n"
  "run(\"handler\", function() local function h(m) depth = depth + 1 return h(m) .. \"\" end return select(2, xpcall(error, h)) end)\n"
  "run(\"handler2\", function() return select(2, xpcall(error, function(m) depth = depth + 1 error(m) end)) end)\n"
  "run(\"errinmeta\", function() return select(2, xpcall(function() return a.x end, function(m) depth = depth + 1 return m end)) end)\n"
  "run(\"incoro\", function() return select(2, coroutine.resume(coroutine.create(function() local function f() depth = depth + 1 return f() + 1 end return f() end))) end)\n"
  "run(\"lua\", function() local function f() depth = depth + 1 return f() + 1 end return f() end)\n"
  "if load(\"local x <close> = nil\") then\n"
  "  run(\"close\", load(\"local function c() depth = depth + 1 local x <close> = setmetatable({}, {__close = function() c() end}) end c()\"))\n"
  "end\n"
;

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  if (luaL_loadstring(L, script) != 0 || lua_pcall(L, 0, 0, 0) != 0)
    printf("failed: %s\n", lua_tostring(L, -1));
  lua_close(L);
  return 0;
}
