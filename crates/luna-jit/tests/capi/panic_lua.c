/* an error in Lua code run with an unprotected lua_call reaches the panic
   function with its position */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  luaL_loadstring(L, "local t = nil\nreturn t.x");
  printf("calling\n");
  fflush(stdout);
  lua_call(L, 0, 0);
  printf("NOT REACHED\n");
  return 0;
}
