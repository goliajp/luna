/* an unprotected error whose object is a number */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"

int main(void) {
  lua_State *L = luaL_newstate();
  lua_pushinteger(L, 42);
  printf("raising\n");
  fflush(stdout);
  lua_error(L);
  printf("NOT REACHED\n");
  return 0;
}
