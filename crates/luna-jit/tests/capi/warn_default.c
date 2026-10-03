/* luaL_newstate's warning function: off in 5.4, on in 5.5, control
   messages, messages in pieces, warn() from Lua, and finalizer errors. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void run(lua_State *L, const char *src) {
  luaL_loadstring(L, src);
  lua_call(L, 0, 0);
}

int main(void) {
#if LUA_VERSION_NUM >= 504
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  setvbuf(stdout, NULL, _IONBF, 0);
  fprintf(stderr, "-- before any control message\n");
  lua_warning(L, "first", 0);
  lua_warning(L, "@on", 0);
  fprintf(stderr, "-- on\n");
  lua_warning(L, "one piece", 0);
  lua_warning(L, "a ", 1);
  fprintf(stderr, "<interleaved>");
  lua_warning(L, "b ", 1);
  lua_warning(L, "c", 0);
  lua_warning(L, "x", 1);
  lua_warning(L, "@off", 0);
  fprintf(stderr, "-- @off as the end of a message\n");
  lua_warning(L, "", 1);
  lua_warning(L, "@off", 0);
  fprintf(stderr, "-- @off after an empty piece\n");
  lua_warning(L, "@unknown", 0);
  lua_warning(L, "@on", 1);
  lua_warning(L, " continued", 0);
  run(L, "warn('from ', 'Lua ', 42)");
  run(L, "setmetatable({}, {__gc = function() error('in gc') end}) collectgarbage()");
  run(L, "setmetatable({}, {__gc = function() error({}) end}) collectgarbage()");
  run(L, "setmetatable({}, {__gc = function() error(12) end}) collectgarbage()");
  lua_warning(L, "@off", 0);
  fprintf(stderr, "-- off\n");
  lua_warning(L, "dropped", 0);
  lua_warning(L, "dropped ", 1);
  lua_warning(L, "@on", 0);
  fprintf(stderr, "-- on again after a dropped continuation\n");
  lua_warning(L, "shown", 0);
  run(L, "warn('@off') warn('dropped') warn('@on') warn('back')");
  run(L, "setmetatable({}, {__gc = function() error('at close') end})");
  lua_close(L);
  printf("done\n");
#else
  printf("no warnings before 5.4\n");
#endif
  return 0;
}
