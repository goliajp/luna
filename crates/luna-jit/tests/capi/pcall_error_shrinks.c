/* a host's lua_pcall catching an error raised in a deep call: from 5.2
   luaD_pcall shrinks the stack and frees CallInfos once the error is set,
   and the count after the call has the smaller stack; 5.2 and 5.3 also
   catch so a finalizer's error, thrown past the message handler */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static const char *plain =
    "local function deep(n) if n > 0 then return deep(n - 1) + 1 end error('plain') end\n"
    "deep(300)\n";

static const char *finalizer =
    "local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end\n"
    "deep(300)\n"
    "setmetatable({}, {__gc = function() error('boom') end})\n"
    "collectgarbage()\n";

static long count(lua_State *L) {
  return (long)lua_gc(L, LUA_GCCOUNT, 0) * 1024 + lua_gc(L, LUA_GCCOUNTB, 0);
}

static void run(const char *what, const char *src) {
  lua_State *L = luaL_newstate();
  long before;
  int st;
  lua_gc(L, LUA_GCSTOP, 0);
  luaL_openlibs(L);
  if (luaL_loadstring(L, src) != 0) {
    printf("%s does not load: %s\n", what, lua_tostring(L, -1));
    lua_close(L);
    return;
  }
  before = count(L);
  st = lua_pcall(L, 0, 0, 0);
  printf("%s %d %ld\n", what, st, count(L) - before);
  lua_close(L);
}

int main(void) {
  run("plain", plain);
  run("finalizer", finalizer);
  return 0;
}
