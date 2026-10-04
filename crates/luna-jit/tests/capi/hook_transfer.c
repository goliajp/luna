/* inside call and return hooks: the transferred values (getinfo 'r',
   5.4+) read with lua_getlocal, the hooked function's locals, and levels
   above it; hooks of C functions */
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
  int i, ev = ar->event;
  const char *name;
  lua_getinfo(L, "nS", ar);
  printf("%s %s %s", ev == LUA_HOOKCALL ? "call" : ev == LUA_HOOKRET ? "ret" : "tail",
         ar->what, ar->name ? ar->name : "?");
#if LUA_VERSION_NUM >= 504
  lua_getinfo(L, "r", ar);
  printf(" ftr=%d ntr=%d [", (int)ar->ftransfer, (int)ar->ntransfer);
  for (i = ar->ftransfer; i < ar->ftransfer + ar->ntransfer; i++) {
    name = lua_getlocal(L, ar, i);
    if (name) { printf(" %s=", name); val(L, -1); lua_pop(L, 1); }
  }
  printf(" ]");
#endif
  /* a Lua function's named locals as it starts; a C function's stack as
     it is called */
  if (ev == LUA_HOOKCALL) {
    printf(" locals:");
    for (i = 1; i <= 4; i++) {
      name = lua_getlocal(L, ar, i);
      if (name && (name[0] != '(' || *ar->what == 'C')) { printf(" %s=", name); val(L, -1); }
      if (name) lua_pop(L, 1);
    }
  }
  if (lua_getstack(L, 1, &up)) {
    lua_getinfo(L, "Sl", &up);
    printf(" | up %s line=%d", up.what, up.currentline);
#if LUA_VERSION_NUM >= 504
    lua_getinfo(L, "r", &up);
    printf(" ftr=%d ntr=%d", (int)up.ftransfer, (int)up.ntransfer);
#endif
  }
  printf("\n");
}

static int cadd(lua_State *L) {
  lua_pushinteger(L, lua_tointeger(L, 1) + lua_tointeger(L, 2));
  lua_pushstring(L, "extra");
  return 2;
}

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_pushcfunction(L, cadd);
  lua_setglobal(L, "cadd");
  st = luaL_loadstring(L,
    "local function f(a, b)\n"
    "  local c = a .. b\n"
    "  return c, 'r2'\n"
    "end\n"
    "local function g(x) local r = f(x, 'y') return r end\n"
    "local r = g('x')\n"
    "local s, t = cadd(1, 2)\n");
  lua_sethook(L, hook, LUA_MASKCALL | LUA_MASKRET, 0);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  lua_sethook(L, NULL, 0, 0);
  printf("status %d\n", st);
  lua_close(L);
  return 0;
}
