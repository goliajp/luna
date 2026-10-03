/* lua_atpanic returns the previous function; a custom panic function sees
   the error on top of the stack, and when it returns the process ends as
   with the default one */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"

static int custom(lua_State *L) {
  printf("custom panic: [%s], %d values on the stack\n", lua_tostring(L, -1), lua_gettop(L));
  fflush(stdout);
  return 0;
}

static int other(lua_State *L) {
  (void)L;
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  lua_CFunction old = lua_atpanic(L, other);
  printf("luaL_newstate has a panic function: %s\n", old != NULL ? "yes" : "no");
  old = lua_atpanic(L, custom);
  printf("the previous one is returned: %s\n", old == other ? "yes" : "no");
  lua_pushstring(L, "a");
  lua_pushstring(L, "b");
  lua_pushstring(L, "the error");
  lua_error(L);
  printf("NOT REACHED\n");
  return 0;
}
