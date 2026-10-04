/* call hooks of tail calls: the event, the called function's name, and
   the level under it with its named locals (5.1 runs the hook before the
   caller's frame is replaced, later versions after) */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void val(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  if (t == LUA_TNUMBER || t == LUA_TSTRING) printf("%s", lua_tostring(L, idx));
  else printf("<%s>", lua_typename(L, t));
}

static void hook(lua_State *L, lua_Debug *ar) {
  lua_Debug up;
  int i;
  lua_getinfo(L, "nSl", ar);
  printf("%s %s:%s line=%d", ar->event == LUA_HOOKCALL ? "call" : "tail", ar->what,
         ar->name ? ar->name : "?", ar->linedefined);
  if (lua_getstack(L, 1, &up)) {
    lua_getinfo(L, "nSl", &up);
    printf(" | up %s:%s cl=%d [", up.what, up.name ? up.name : "?", up.currentline);
    for (i = 1; i <= 6; i++) {
      const char *name = lua_getlocal(L, &up, i);
      if (name == NULL) break;
      if (name[0] != '(') {
        printf(" %s=", name);
        val(L, -1);
      }
      lua_pop(L, 1);
    }
    printf(" ]");
  }
  printf("\n");
}

static int cfun(lua_State *L) {
  lua_pushinteger(L, lua_tointeger(L, 1) * 2);
  return 1;
}

static const char *script =
  "local function leaf(x) return x end\n"
  "local function mid(x)\n"
  "  local m = 'mid'\n"
  "  return leaf(x)\n"
  "end\n"
  "local function top()\n"
  "  local t = 'top'\n"
  "  return mid(1)\n"
  "end\n"
  "local function tail_c(v)\n"
  "  local c = 'c'\n"
  "  return cfun(v)\n"
  "end\n"
  "top()\n"
  "tail_c(2)\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_pushcfunction(L, cfun);
  lua_setglobal(L, "cfun");
  st = luaL_loadstring(L, script);
  lua_sethook(L, hook, LUA_MASKCALL, 0);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  lua_sethook(L, NULL, 0, 0);
  printf("status %d\n", st);
  lua_close(L);
  return 0;
}
