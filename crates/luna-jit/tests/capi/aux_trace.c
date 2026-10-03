/* luaL_traceback: from C and Lua frames, with and without a message, of
   another thread, deep stacks that skip levels, tail calls, and the names
   it finds for functions */
#include "aux_common.h"

#if LUA_VERSION_NUM >= 502
static int f_tb(lua_State *L) {
  const char *msg = lua_isnoneornil(L, 1) ? NULL : lua_tostring(L, 1);
  int level = (int)luaL_optinteger(L, 2, 1);
  int top = lua_gettop(L);
  luaL_traceback(L, L, msg, level);
  printf("traceback pushed %d\n", lua_gettop(L) - top);
  return 1;
}

static int f_co_tb(lua_State *L) {
  lua_State *co = lua_tothread(L, 1);
  luaL_traceback(L, co, "co", 0);
  return 1;
}

static int f_handler(lua_State *L) {
  luaL_traceback(L, L, lua_tostring(L, 1), 1);
  return 1;
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
#if LUA_VERSION_NUM >= 502
  lua_register(L, "tb", f_tb);
  lua_register(L, "cotb", f_co_tb);
  lua_settop(L, 0);
  /* from the host: no level */
  luaL_traceback(L, L, "host", 0);
  show_from(L, "host", 1);
  lua_settop(L, 0);
  luaL_traceback(L, L, NULL, 1);
  show_from(L, "host no msg", 1);
  lua_settop(L, 0);
  dochunk(L, "print(tb('msg'))");
  dochunk(L, "print(tb(nil, 0))");
  dochunk(L,
          "local function inner() return tb('in inner') end\n"
          "local function outer() local s = inner() return s end\n"
          "print(outer())");
  dochunk(L,
          "local t = {}\n"
          "function t.field() return tb('field') end\n"
          "function t:method() return tb('method') end\n"
          "print(t.field())\n"
          "print(t:method())");
  dochunk(L,
          "function glob() return tb('global') end\n"
          "print((glob()))");
  dochunk(L,
          "local function tail() return tb('tail') end\n"
          "local function caller() return tail() end\n"
          "print(caller())");
  dochunk(L,
          "local function rec(n) if n == 0 then return tb('deep') end local s = rec(n - 1) return s end\n"
          "print(rec(30))");
  dochunk(L,
          "local function rec(n) if n == 0 then return tb('edge') end local s = rec(n - 1) return s end\n"
          "print(rec(19))\n"
          "print(rec(20))\n"
          "print(rec(21))");
  dochunk(L, "print(tb('high level', 50))");
  dochunk(L, "print(pcall(string.rep))");
  dochunk(L, "print(select(2, xpcall(function() local x = nil; x() end, debug.traceback)))");
  dochunk(L, "local s = string.format print((pcall(function() return tb('lib') end)))");
  dochunk(L, "print(table.concat({pcall(function() return string.gsub('a', 'a', tb) end)}, ' '))");
  /* another thread */
  dochunk(L,
          "local co = coroutine.create(function() local function y() coroutine.yield() end y() end)\n"
          "coroutine.resume(co)\n"
          "print(cotb(co))");
  /* as a message handler */
  lua_settop(L, 0);
  lua_pushcfunction(L, f_handler);
  luaL_loadstring(L, "local function bad() error('boom') end bad()");
  printf("handler: status=%d\n", lua_pcall(L, 0, 0, 1));
  printf("%s\n", lua_tostring(L, -1));
  lua_settop(L, 0);
#else
  printf("no luaL_traceback in 5.1\n");
#endif
  printf("top at end=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
