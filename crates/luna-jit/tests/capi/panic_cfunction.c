/* an error raised in a C function called with an unprotected lua_call */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static int fails(lua_State *L) {
  lua_pushstring(L, "from a C function");
  return lua_error(L);
}

int main(void) {
  lua_State *L = luaL_newstate();
  lua_pushcfunction(L, fails);
  printf("calling\n");
  fflush(stdout);
  lua_call(L, 0, 0);
  printf("NOT REACHED\n");
  return 0;
}
