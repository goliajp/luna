/* lua_setwarnf: a host warning function sees every piece, including
   finalizer errors; NULL turns warnings off; a warning function that
   replaces itself; one that raises; lua_newstate has none. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#ifndef LUA_OK
#define LUA_OK 0
#endif

#if LUA_VERSION_NUM >= 504
static void show(void *ud, const char *msg, int tocont) {
  printf("  warn(%s): [%s]%s\n", (const char *)ud, msg, tocont ? " ..." : "");
}

static void second(void *ud, const char *msg, int tocont);

/* switches to `second` after a message ends, as lauxlib's functions do */
static void first(void *ud, const char *msg, int tocont) {
  lua_State *L = (lua_State *)ud;
  printf("  first: [%s]%s\n", msg, tocont ? " ..." : "");
  if (!tocont)
    lua_setwarnf(L, second, L);
}

static void second(void *ud, const char *msg, int tocont) {
  lua_State *L = (lua_State *)ud;
  printf("  second: [%s]%s\n", msg, tocont ? " ..." : "");
  if (!tocont)
    lua_setwarnf(L, first, L);
}

static void raising(void *ud, const char *msg, int tocont) {
  lua_State *L = (lua_State *)ud;
  (void)tocont;
  lua_pushfstring(L, "warning raised on '%s'", msg);
  lua_error(L);
}

static int warn_in_c(lua_State *L) {
  lua_warning(L, "from C", 0);
  printf("  NOT REACHED after lua_warning\n");
  return 0;
}

static void *alloc(void *ud, void *ptr, size_t osize, size_t nsize) {
  (void)ud;
  (void)osize;
  if (nsize == 0) {
    free(ptr);
    return NULL;
  }
  return realloc(ptr, nsize);
}

static void run(lua_State *L, const char *src) {
  int st;
  luaL_loadstring(L, src);
  st = lua_pcall(L, 0, 0, 0);
  if (st != LUA_OK) {
    printf("  error %d: %s\n", st, lua_tostring(L, -1));
    lua_pop(L, 1);
  }
}
#endif

int main(void) {
#if LUA_VERSION_NUM >= 504
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  printf("a host warning function\n");
  lua_setwarnf(L, show, "host");
  lua_warning(L, "@on", 0);
  lua_warning(L, "a", 1);
  lua_warning(L, "b", 0);
  run(L, "warn('x', 'y', 'z')");
  run(L, "warn('@off')");
  run(L, "setmetatable({}, {__gc = function() error('gc error') end}) collectgarbage()");
  run(L, "setmetatable({}, {__gc = function() error(false) end}) collectgarbage()");
  run(L, "warn()");
  run(L, "warn('a', {})");
  printf("NULL turns warnings off\n");
  lua_setwarnf(L, NULL, NULL);
  lua_warning(L, "@on", 0);
  lua_warning(L, "not shown", 0);
  run(L, "warn('@on') warn('not shown')");
  printf("functions that replace themselves\n");
  lua_setwarnf(L, first, L);
  lua_warning(L, "one", 0);
  lua_warning(L, "two", 1);
  lua_warning(L, "three", 0);
  run(L, "warn('four') warn('five')");
  printf("a warning function that raises\n");
  lua_setwarnf(L, raising, L);
  lua_pushcfunction(L, warn_in_c);
  printf("  pcall status %d", lua_pcall(L, 0, 0, 0));
  printf(": %s\n", lua_tostring(L, -1));
  lua_pop(L, 1);
  run(L, "warn('from Lua')");
  run(L, "local ok, e = pcall(warn, 'in pcall') print('  pcall(warn):', ok, e)");
  lua_setwarnf(L, show, "at close");
  run(L, "setmetatable({}, {__gc = function() error('at close') end})");
  lua_close(L);

  printf("lua_newstate has no warning function\n");
#if LUA_VERSION_NUM >= 505
  L = lua_newstate(alloc, NULL, 0);
#else
  L = lua_newstate(alloc, NULL);
#endif
  luaL_openlibs(L);
  lua_warning(L, "@on", 0);
  lua_warning(L, "not shown", 0);
  run(L, "warn('@on') warn('not shown either')");
  run(L, "setmetatable({}, {__gc = function() error('gc error') end}) collectgarbage()");
  lua_close(L);
  printf("done\n");
#else
  printf("no warnings before 5.4\n");
#endif
  return 0;
}
