/* 5.4+: an unprotected error closes the thread's to-be-closed slots
   before the panic function runs; a failing __close replaces the error */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static int custom(lua_State *L) {
  printf("panic: [%s], %d values on the stack\n", lua_tostring(L, -1), lua_gettop(L));
  fflush(stdout);
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  lua_atpanic(L, custom);
#if LUA_VERSION_NUM >= 504
  luaL_loadstring(L, "return setmetatable({}, {__close = function(_, e) print('close 1:', e) end}),"
                     " setmetatable({}, {__close = function(_, e) print('close 2:', e) error('replaced', 0) end})");
  lua_call(L, 0, 2);
  lua_toclose(L, 1);
  lua_toclose(L, 2);
#endif
  lua_pushstring(L, "original");
  lua_error(L);
  printf("NOT REACHED\n");
  return 0;
}
