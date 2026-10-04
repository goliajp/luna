/* the locals of vararg functions as lua_getlocal lists them: 5.1 (with
   LUA_COMPAT_VARARG, as PUC's makefile builds it) declares a local `arg`
   after the fixed parameters of every vararg function, which holds a table
   of the extra arguments only when the body does not use `...` */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void val(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  if (t == LUA_TNUMBER || t == LUA_TSTRING) printf("%s", lua_tostring(L, idx));
  else if (t == LUA_TTABLE) {
    lua_getfield(L, idx, "n");
    printf("<table n=%s>", lua_isnil(L, -1) ? "nil" : lua_tostring(L, -1));
    lua_pop(L, 1);
  } else printf("<%s>", lua_typename(L, t));
}

/* the named locals of the function that called this one */
static int show(lua_State *L) {
  lua_Debug ar;
  int i;
  printf("%s:", lua_tostring(L, 1));
  if (!lua_getstack(L, 1, &ar)) return 0;
  for (i = 1; i <= 8; i++) {
    const char *name = lua_getlocal(L, &ar, i);
    if (name == NULL) break;
    if (name[0] != '(') {
      printf(" %d:%s=", i, name);
      val(L, -1);
    }
    lua_pop(L, 1);
  }
  printf("\n");
  return 0;
}

static const char *script =
  "arg = 'global'\n"
  "local function uses_dots(a, ...)\n"
  "  local n = select('#', ...)\n"
  "  show('uses_dots')\n"
  "  return arg, n\n"
  "end\n"
  "local function no_dots(a, ...)\n"
  "  local x = 'x'\n"
  "  show('no_dots')\n"
  "  return type(arg)\n"
  "end\n"
  "local function only_dots(...)\n"
  "  show('only_dots')\n"
  "  return ...\n"
  "end\n"
  "print('uses_dots', uses_dots(1, 2, 3))\n"
  "print('no_dots', no_dots(1, 2, 3))\n"
  "print('only_dots', only_dots('a', 'b'))\n"
  "print('dots in a nested function', (function(...)\n"
  "  local inner = function(...) return ... end\n"
  "  show('outer')\n"
  "  return type(arg)\n"
  "end)(7, 8))\n";

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_pushcfunction(L, show);
  lua_setglobal(L, "show");
  st = luaL_loadstring(L, script);
  if (st == 0) st = lua_pcall(L, 0, 0, 0);
  printf("status %d %s\n", st, st ? lua_tostring(L, -1) : "");
  lua_close(L);
  return 0;
}
