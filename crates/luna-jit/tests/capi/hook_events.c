/* lua_sethook with C hooks: call, return, line, count and tail events,
   what the hook sees through lua_getinfo, lua_gethook / lua_gethookmask /
   lua_gethookcount, sharing the thread's hook with debug.sethook, and
   hooks on coroutines */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char *const evname[] = {"call", "ret", "line", "count", "tail"};
static int nevents;

static void hook(lua_State *L, lua_Debug *ar) {
  int ev = ar->event;
  if (nevents++ > 200) return;
  if (!lua_getinfo(L, "nSl", ar)) printf("getinfo failed\n");
  printf("  %s", evname[ev]);
  if (ev == LUA_HOOKLINE) printf(" @%d", ar->currentline);
  printf(" %s:%s %s cl=%d\n", ar->what, ar->short_src, ar->name ? ar->name : "?", ar->currentline);
}

/* only what each event says, without getinfo */
static void bare(lua_State *L, lua_Debug *ar) {
  (void)L;
  if (nevents++ > 200) return;
  printf("  %s line=%d\n", evname[ar->event], ar->currentline);
}

static int cf(lua_State *L) {
  lua_pushinteger(L, 7);
  return 1;
}

static void run(lua_State *L, const char *src) {
  int st = luaL_loadstring(L, src);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  if (st) printf("  error %d: %s\n", st, lua_tostring(L, -1));
  lua_settop(L, 0);
}

static void info(lua_State *L, const char *what) {
  lua_Hook h = lua_gethook(L);
  printf("%s: hook=%s mask=%d count=%d\n", what,
         h == NULL ? "none" : h == hook ? "hook" : h == bare ? "bare" : "other",
         lua_gethookmask(L), lua_gethookcount(L));
}

static void raiser(lua_State *L, lua_Debug *ar) {
  lua_getinfo(L, "l", ar);
  if (ar->currentline == 3) {
    lua_pushstring(L, "hook error");
    lua_error(L);
  }
}

static void calls_lua(lua_State *L, lua_Debug *ar) {
  (void)ar;
  lua_getglobal(L, "inhook");
  lua_call(L, 0, 0);
}

static const char *body =
  "local function f(x)\n"
  "  return x + 1\n"
  "end\n"
  "local function t(x) return f(x) end\n"
  "local y = t(1)\n"
  "y = cf()\n"
  "for i = 1, 2 do y = y + i end\n";

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  lua_pushcfunction(L, cf);
  lua_setglobal(L, "cf");
  info(L, "initial");

  printf("call+return\n");
  lua_sethook(L, hook, LUA_MASKCALL | LUA_MASKRET, 0);
  info(L, "set");
  run(L, body);
  lua_sethook(L, NULL, 0, 0);
  printf("line\n");
  lua_sethook(L, hook, LUA_MASKLINE, 0);
  run(L, body);
  lua_sethook(L, NULL, 0, 0);
  printf("count 5\n");
  lua_sethook(L, bare, LUA_MASKCOUNT, 5);
  info(L, "set");
  run(L, body);
  lua_sethook(L, bare, LUA_MASKCOUNT, 0);
  info(L, "count 0");
  run(L, "local a = 1 a = a + 1");
  lua_sethook(L, bare, 0, 3);
  info(L, "mask 0");
  lua_sethook(L, NULL, LUA_MASKLINE, 3);
  info(L, "func NULL");

  printf("debug.sethook\n");
  run(L, "debug.sethook(function(e, l) print('  lua hook', e, l) end, 'l')\n"
         "local a = 1\n"
         "debug.sethook()");
  run(L, "debug.sethook(function(e, l) print('  lua hook', e, l) end, 'l')");
  info(L, "lua hook");
  {
    lua_Hook old = lua_gethook(L);
    int mask = lua_gethookmask(L), count = lua_gethookcount(L);
    lua_sethook(L, bare, LUA_MASKCALL, 0);
    info(L, "c over lua");
    run(L, "local h, m, c = debug.gethook() R = tostring(h) .. ' ' .. m .. ' ' .. c");
    run(L, "print(R)");
    lua_sethook(L, old, mask, count);
    info(L, "restored");
    run(L, "local h, m, c = debug.gethook() print(type(h), m, c)\nlocal z = 1");
    lua_sethook(L, NULL, 0, 0);
    run(L, "print('cleared', type((debug.gethook())))");
  }

  printf("hook error\n");
  lua_sethook(L, raiser, LUA_MASKLINE, 0);
  run(L, "local a = 1\nlocal b = 2\nlocal c = 3\nprint('not reached')");
  lua_sethook(L, NULL, 0, 0);
  run(L, "local ok, e = pcall(function() debug.sethook(error, 'l') local a = 1 end)\n"
         "debug.sethook() print('lua hook error', ok, e)");

  printf("hook calls Lua\n");
  run(L, "function inhook() local i = debug.getinfo(1, 'n') print('  inhook', i.namewhat, i.name) end");
  lua_sethook(L, calls_lua, LUA_MASKCALL, 0);
  run(L, "local function g() end g()");
  lua_sethook(L, NULL, 0, 0);

  printf("coroutines\n");
  run(L, "CO = coroutine.create(function() local a = 1\ncoroutine.yield()\nlocal b = 2 end)\n"
         "coroutine.resume(CO)");
  lua_getglobal(L, "CO");
  {
    lua_State *co = lua_tothread(L, -1);
    lua_sethook(co, hook, LUA_MASKLINE | LUA_MASKRET, 0);
    info(co, "co");
    info(L, "main");
    run(L, "print('resume', coroutine.resume(CO))");
    run(L, "print(debug.gethook(CO))");
  }
  lua_sethook(L, bare, LUA_MASKLINE, 0);
  run(L, "local co = coroutine.create(function()\nlocal x = 1\nend)\ncoroutine.resume(co)");
  lua_sethook(L, NULL, 0, 0);
  lua_close(L);
  return 0;
}
