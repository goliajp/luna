/* Unbounded recursion through C functions ends in a Lua error that
   lua_pcall catches, as PUC's C-call limit makes it, and leaves the state
   usable: a C function calling Lua that calls it back, a C function
   calling itself, and protected calls nested without end. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static int depth;

static int c_to_lua(lua_State *L) {
  depth++;
  lua_getglobal(L, "back");
  lua_call(L, 0, 0);
  return 0;
}

static int c_self(lua_State *L) {
  depth++;
  lua_pushcfunction(L, c_self);
  lua_call(L, 0, 0);
  return 0;
}

static int c_pcall(lua_State *L) {
  depth++;
  lua_pushcfunction(L, c_pcall);
  if (lua_pcall(L, 0, 0, 0) != 0)
    return lua_error(L);
  return 0;
}

static void run(lua_State *L, const char *name, lua_CFunction f) {
  int st;
  depth = 0;
  lua_pushcfunction(L, f);
  st = lua_pcall(L, 0, 0, 0);
  printf("%s: status %d, nested %s, %s\n", name, st, depth > 10 ? "yes" : "no",
         lua_tostring(L, -1));
  lua_settop(L, 0);
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  lua_register(L, "c_to_lua", c_to_lua);
  if (luaL_dostring(L, "function back() c_to_lua() end") != 0) {
    printf("setup failed: %s\n", lua_tostring(L, -1));
    return 1;
  }
  run(L, "c_to_lua", c_to_lua);
  run(L, "c_self", c_self);
  run(L, "c_pcall", c_pcall);
  luaL_dostring(L, "return 20 + 22");
  printf("after: %d\n", (int)lua_tointeger(L, -1));
  lua_close(L);
  return 0;
}
