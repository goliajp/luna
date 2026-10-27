/* the name luaL_argerror gives a function called from C, which has no name
   of its own: each function is a global f<i> and also the field f of a
   global table t<i>. 5.2 searches the globals two levels deep in their
   hash order, so which of the two names it reaches first depends on the
   seed; later versions search the loaded modules, where only f<i> is two
   levels deep; 5.1 does not search. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#define N 16

static int number_arg(lua_State *L) {
  luaL_checknumber(L, 1);
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  int i;
  luaL_openlibs(L);
  for (i = 1; i <= N; i++) {
    char name[16];
    lua_pushinteger(L, i);
    lua_pushcclosure(L, number_arg, 1);
    lua_newtable(L);
    lua_pushvalue(L, -2);
    lua_setfield(L, -2, "f");
    sprintf(name, "t%d", i);
    lua_setglobal(L, name);
    sprintf(name, "f%d", i);
    lua_setglobal(L, name);
  }
  for (i = 1; i <= N; i++) {
    char name[16];
    sprintf(name, "f%d", i);
    lua_getglobal(L, name);
    lua_pushstring(L, "x");
    if (lua_pcall(L, 1, 0, 0) != 0) printf("%s: %s\n", name, lua_tostring(L, -1));
    lua_settop(L, 0);
  }
  lua_close(L);
  return 0;
}
