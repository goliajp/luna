/* a line or count hook yields with lua_yield(L, 0): the coroutine
   suspends before the instruction, and the resume runs it; yielding where
   the thread cannot yield is an error */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static int nev;

static void yline(lua_State *L, lua_Debug *ar) {
  lua_getinfo(L, "l", ar);
  printf("  hook %s line=%d\n", ar->event == LUA_HOOKLINE ? "line" : "count", ar->currentline);
  if (nev++ < 40 && (ar->currentline % 2 == 0 || ar->event == LUA_HOOKCOUNT))
    lua_yield(L, 0);
}

static void ymain(lua_State *L, lua_Debug *ar) {
  (void)ar;
  lua_sethook(L, NULL, 0, 0);
  lua_yield(L, 0);
}

static void run(lua_State *L, const char *src) {
  int st = luaL_loadstring(L, src);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  if (st) printf("  error %d: %s\n", st, lua_tostring(L, -1));
  lua_settop(L, 0);
}

static int sethook(lua_State *L) {
  lua_State *co = lua_tothread(L, 1);
  int mask = (int)lua_tointeger(L, 2);
  lua_sethook(co, yline, mask, (int)lua_tointeger(L, 3));
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  lua_pushcfunction(L, sethook);
  lua_setglobal(L, "csethook");
  printf("line yields\n");
  run(L,
    "local co = coroutine.create(function(a)\n"
    "  local x = a\n"
    "  x = x + 1\n"
    "  x = x * 2\n"
    "  return x\n"
    "end)\n"
    "csethook(co, 4, 0)\n"
    "for i = 1, 12 do\n"
    "  local r = {coroutine.resume(co, 10)}\n"
    "  print(i, coroutine.status(co), r[1], r[2], #r)\n"
    "  if coroutine.status(co) == 'dead' then break end\n"
    "end\n");
  nev = 0;
  printf("count yields\n");
  run(L,
    "local co = coroutine.create(function()\n"
    "  local s = 0\n"
    "  for i = 1, 3 do s = s + i end\n"
    "  return s\n"
    "end)\n"
    "csethook(co, 8, 4)\n"
    "for i = 1, 30 do\n"
    "  local ok, v = coroutine.resume(co)\n"
    "  print(i, coroutine.status(co), ok, v)\n"
    "  if coroutine.status(co) == 'dead' then break end\n"
    "end\n");
  nev = 0;
  printf("wrap\n");
  run(L,
    "local w = coroutine.wrap(function()\n"
    "  csethook(coroutine.running(), 4, 0)\n"
    "  local a = 1\n"
    "  a = 2\n"
    "  return 'done'\n"
    "end)\n"
    "for i = 1, 6 do print(w()) end\n");
#if LUA_VERSION_NUM >= 502
  printf("main thread\n");
  lua_sethook(L, ymain, LUA_MASKLINE, 0);
  run(L, "local a = 1");
#endif
  lua_close(L);
  return 0;
}
