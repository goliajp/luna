/* an unprotected lua_error with a string: luaL_newstate's panic function
   reports it and the process ends (5.1: exit(EXIT_FAILURE), later abort) */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"

int main(void) {
  lua_State *L = luaL_newstate();
  lua_pushinteger(L, 1);
  lua_pushstring(L, "unprotected");
  printf("raising\n");
  fflush(stdout);
  lua_error(L);
  printf("NOT REACHED\n");
  return 0;
}
