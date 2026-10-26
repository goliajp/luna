/* the count once the libraries are open, all of them and the package
   library alone: what each opener makes, the registry's table of C
   libraries made once */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void show(lua_State *L, const char *what) {
  printf("%s %d %d\n", what, lua_gc(L, LUA_GCCOUNT, 0), lua_gc(L, LUA_GCCOUNTB, 0));
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  show(L, "all");
  lua_close(L);
  L = luaL_newstate();
#if LUA_VERSION_NUM == 501
  lua_pushcfunction(L, luaopen_package);
  lua_pushstring(L, "package");
  lua_call(L, 1, 0);
#else
  luaL_requiref(L, "package", luaopen_package, 1);
  lua_pop(L, 1);
#endif
  show(L, "package");
  lua_close(L);
  return 0;
}
