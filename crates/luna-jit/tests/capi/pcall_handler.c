/* lua_pcall with a message handler, per dialect: status and error object */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void show(lua_State *L, const char *name, int st) {
  int t = lua_type(L, -1);
  printf("%s: status=%d top=%d type=%s", name, st, lua_gettop(L), lua_typename(L, t));
  if (t == LUA_TSTRING) printf(" value=[%s]", lua_tostring(L, -1));
  if (t == LUA_TNUMBER) printf(" value=[%s]", lua_tostring(L, -1));
  printf("\n");
  lua_settop(L, 0);
}

/* push the function a chunk returns */
static void pushfn(lua_State *L, const char *src) {
  if (luaL_loadstring(L, src) != 0 || lua_pcall(L, 0, 1, 0) != 0) {
    printf("setup failed: %s\n", lua_tostring(L, -1));
  }
}

static int c_handler(lua_State *L) {
  lua_pushstring(L, "C:");
  lua_pushvalue(L, 1);
  lua_concat(L, 2);
  return 1;
}

static int c_raiser(lua_State *L) {
  lua_pushstring(L, "craise");
  return lua_error(L);
}

/* a C function that makes its own lua_pcall with a handler */
static int c_nested(lua_State *L) {
  int st;
  pushfn(L, "return function(m) return 'inner:' .. tostring(m) end");
  pushfn(L, "return function() error('deep', 0) end");
  st = lua_pcall(L, 0, 0, -2);
  lua_pushinteger(L, st);
  lua_insert(L, -2);
  return 2;
}

static int c_getbad(lua_State *L) {
  lua_getglobal(L, "bad");
  lua_pushstring(L, "after");
  return 1;
}

static int c_setbad(lua_State *L) {
  lua_pushstring(L, "v");
  lua_setglobal(L, "bad");
  lua_pushstring(L, "after");
  return 1;
}

static void run(lua_State *L, const char *name, const char *h, const char *f, int nargs) {
  int st, i;
  pushfn(L, h);
  pushfn(L, f);
  for (i = 0; i < nargs; i++) lua_pushinteger(L, i + 1);
  st = lua_pcall(L, nargs, 1, 1);
  show(L, name, st);
}

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  printf("LUA_ERRRUN=%d LUA_ERRERR=%d\n", LUA_ERRRUN, LUA_ERRERR);
  run(L, "returns", "return function(m) return 'H:' .. m end", "return function() error('boom', 0) end", 0);
  run(L, "returns_table", "return function(m) return {m} end", "return function() error('boom', 0) end", 0);
  run(L, "returns_nothing", "return function(m) end", "return function() error('boom', 0) end", 0);
  run(L, "returns_two", "return function(m) return 'a', 'b' end", "return function() error('boom', 0) end", 0);
  run(L, "nonstring_err", "return function(m) return type(m) end", "return function() error({}) end", 0);
  run(L, "nil_err", "return function(m) return type(m) end", "return function() error() end", 0);
  run(L, "runtime_err", "return function(m) return 'H:' .. m end", "return function() local t = nil; return t.x end", 0);
  run(L, "args", "return function(m) return 'H:' .. m end", "return function(a, b) error(a + b, 0) end", 2);
  run(L, "always_errors", "return function(m) error('again', 0) end", "return function() error('boom', 0) end", 0);
  run(L, "errors_once",
      "local n = 0; return function(m) n = n + 1; if n == 1 then error('again', 0) end; return 'H' .. n .. ':' .. m end",
      "return function() error('boom', 0) end", 0);
  run(L, "runtime_err_in_handler", "return function(m) local t = nil; return t.x end", "return function() error('boom', 0) end", 0);
  run(L, "traceback", "return debug.traceback", "return function() error('boom') end", 0);
  run(L, "no_error", "return function(m) return 'H:' .. m end", "return function() return 7 end", 0);
  /* handler is not a function */
  lua_pushinteger(L, 42);
  pushfn(L, "return function() error('boom', 0) end");
  st = lua_pcall(L, 0, 1, 1);
  show(L, "handler_number", st);
  /* C function handler */
  lua_pushcfunction(L, c_handler);
  pushfn(L, "return function() error('boom', 0) end");
  st = lua_pcall(L, 0, 1, 1);
  show(L, "c_handler", st);
  /* error raised by a C function, handler sees it */
  pushfn(L, "return function(m) return 'H:' .. m end");
  lua_pushcfunction(L, c_raiser);
  st = lua_pcall(L, 0, 1, 1);
  show(L, "c_raiser", st);
  /* negative handler index: [h, f, arg] -> msgh = -3 */
  pushfn(L, "return function(m) return 'neg:' .. m end");
  pushfn(L, "return function(a) error('x' .. a, 0) end");
  lua_pushinteger(L, 5);
  st = lua_pcall(L, 1, 1, -3);
  show(L, "negative_index", st);
  /* handler below other values: [h, junk, f] -> msgh = 1 */
  pushfn(L, "return function(m) return 'deep:' .. m end");
  lua_pushinteger(L, 99);
  pushfn(L, "return function() error('y', 0) end");
  st = lua_pcall(L, 0, 1, 1);
  printf("handler_below: status=%d top=%d [%s] [%s]\n", st, lua_gettop(L), lua_tostring(L, 2), lua_tostring(L, 3));
  lua_settop(L, 0);
  /* no handler */
  pushfn(L, "return function() error('boom', 0) end");
  st = lua_pcall(L, 0, 1, 0);
  show(L, "no_handler", st);
  pushfn(L, "return function() error() end");
  st = lua_pcall(L, 0, 1, 0);
  show(L, "no_handler_nil", st);
  /* lua_pcall with a handler from inside a C function */
  lua_pushcfunction(L, c_nested);
  st = lua_pcall(L, 0, 2, 0);
  printf("nested: status=%d inner=%s [%s]\n", st, lua_tostring(L, 1), lua_tostring(L, 2));
  lua_settop(L, 0);
  /* inner pcall caught an error-in-error-handling, then a plain error */
  run(L, "inner_errerr_then_plain",
      "return function(m) return 'H:' .. tostring(m) .. '|' .. tostring(inner) end",
      "return function() local _; _, inner = xpcall(function() error('a') end, function() error('b') end); error('plain', 0) end", 0);
  /* handler returns the errerr text itself */
  run(L, "returns_errerr_text", "return function(m) return 'error in error handling' end",
      "return function() error('boom', 0) end", 0);
  /* globals read and written through _G's metamethods */
  if (luaL_dostring(L, "setmetatable(_G, {__index = function(t, k) if k == 'bad' then error('no global ' .. k) end return 'idx:' .. k end, __newindex = function(t, k, v) if k == 'bad' then error('cannot set ' .. k, 0) end rawset(t, k, v .. '!') end})") != 0)
    printf("setup failed\n");
  lua_getglobal(L, "missing");
  show(L, "getglobal_mm", 0);
  lua_pushstring(L, "v");
  lua_setglobal(L, "newg");
  lua_getglobal(L, "newg");
  show(L, "setglobal_mm", 0);
  lua_pushcfunction(L, c_getbad);
  st = lua_pcall(L, 0, 1, 0);
  show(L, "getglobal_error", st);
  lua_pushcfunction(L, c_setbad);
  st = lua_pcall(L, 0, 1, 0);
  show(L, "setglobal_error", st);
  pushfn(L, "return function(m) return 'H:' .. m end");
  lua_pushcfunction(L, c_setbad);
  st = lua_pcall(L, 0, 1, 1);
  show(L, "setglobal_error_handler", st);
  lua_close(L);
  return 0;
}
