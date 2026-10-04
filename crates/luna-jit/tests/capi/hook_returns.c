/* return hooks: what lua_getlocal reads in the return hook of a C
   function (its arguments, what it pushed, its results) and of a library
   function, the named locals of a Lua function and of the main chunk at
   their final return, and the transferred values (getinfo 'r', 5.4+) */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void val(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  if (t == LUA_TNUMBER || t == LUA_TSTRING) printf("%s", lua_tostring(L, idx));
  else if (t == LUA_TBOOLEAN) printf("%s", lua_toboolean(L, idx) ? "true" : "false");
  else printf("<%s>", lua_typename(L, t));
}

static void hook(lua_State *L, lua_Debug *ar) {
  int i;
  lua_getinfo(L, "nS", ar);
  printf("ret %s %s", ar->what, ar->name ? ar->name : "?");
#if LUA_VERSION_NUM >= 504
  lua_getinfo(L, "r", ar);
  printf(" ftr=%d ntr=%d", (int)ar->ftransfer, (int)ar->ntransfer);
#endif
  printf(" [");
  for (i = 1; i <= 12; i++) {
    const char *name = lua_getlocal(L, ar, i);
    if (name == NULL) break;
    /* a Lua function's temporaries are the compiler's choice */
    if (*ar->what == 'C' || name[0] != '(') {
      printf(" %d:%s=", i, name);
      val(L, -1);
    }
    lua_pop(L, 1);
  }
  printf(" ]\n");
}

/* two results over a temporary */
static int cadd(lua_State *L) {
  lua_Integer a = luaL_checkinteger(L, 1), b = luaL_checkinteger(L, 2);
  lua_pushstring(L, "temp");
  lua_pushinteger(L, a + b);
  lua_pushstring(L, "extra");
  return 2;
}

/* no results */
static int cnone(lua_State *L) {
  lua_pushstring(L, "left");
  return 0;
}

/* returns its own argument */
static int cself(lua_State *L) {
  return 1;
}

static const char *script =
  "local a, b = 1, 'two'\n"
  "local function f(x, y)\n"
  "  local z = x .. y\n"
  "  return z, 'r2'\n"
  "end\n"
  "local function g(p, ...)\n"
  "  local n = select('#', ...)\n"
  "end\n"
  "local r1 = f('p', 'q')\n"
  "local r2, r3 = cadd(3, 4)\n"
  "cnone(5, 6)\n"
  "local r4 = cself('me')\n"
  "local r5 = type(r1)\n"
  "local r6 = rawequal(a, b)\n"
  "g(1, 2, 3)\n"
  "do local inner = 'i' end\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_pushcfunction(L, cadd);
  lua_setglobal(L, "cadd");
  lua_pushcfunction(L, cnone);
  lua_setglobal(L, "cnone");
  lua_pushcfunction(L, cself);
  lua_setglobal(L, "cself");
  st = luaL_loadstring(L, script);
  lua_sethook(L, hook, LUA_MASKRET, 0);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  lua_sethook(L, NULL, 0, 0);
  printf("status %d\n", st);
  lua_close(L);
  return 0;
}
