/* lua_close: finalizers run (and their errors are dropped), to-be-closed
   slots of the main thread close first (5.4+), lua_gc inside a finalizer
   run by lua_close, and lua_close on a coroutine's lua_State. */
#include <stdio.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM >= 504
#define GC0(L, w) lua_gc(L, w)
#else
#define GC0(L, w) lua_gc(L, w, 0)
#endif

static int count_in_finalizer(lua_State *L) {
  int r = GC0(L, LUA_GCCOUNT);
  printf("  lua_gc in a finalizer at close: %s\n", r < 0 ? "-1" : ">= 0");
  return 0;
}

static void run(lua_State *L, const char *src) {
  luaL_loadstring(L, src);
  lua_call(L, 0, 0);
}

static void make_finalized(lua_State *L, const char *name, int fail) {
  char src[300];
#if LUA_VERSION_NUM == 501
  snprintf(src, sizeof src,
           "local u = newproxy(true) getmetatable(u).__gc = function() print('  finalize %s') %s end keep_%s = u",
           name, fail ? "error('fin error')" : "", name);
#else
  snprintf(src, sizeof src,
           "keep_%s = setmetatable({}, {__gc = function() print('  finalize %s') %s end})", name,
           name, fail ? "error('fin error')" : "");
#endif
  run(L, src);
}

static void one_state(int with_coroutine) {
  lua_State *L = luaL_newstate();
  lua_State *co;
  luaL_openlibs(L);
  printf("state %d\n", with_coroutine);
  make_finalized(L, "first", 0);
  make_finalized(L, "failing", 1);
  make_finalized(L, "last", 0);
  lua_pushcfunction(L, count_in_finalizer);
  lua_setglobal(L, "count_in_finalizer");
#if LUA_VERSION_NUM == 501
  run(L, "local u = newproxy(true) getmetatable(u).__gc = count_in_finalizer keep_count = u");
#else
  run(L, "keep_count = setmetatable({}, {__gc = count_in_finalizer})");
#endif
#if LUA_VERSION_NUM >= 504
  run(L, "closer = setmetatable({}, {__close = function(_, e) print('  close slot, error:', e) end})");
  lua_getglobal(L, "closer");
  lua_toclose(L, -1);
  run(L, "failing_closer = setmetatable({}, {__close = function() print('  failing close') error('close error') end})");
  lua_getglobal(L, "failing_closer");
  lua_toclose(L, -1);
  lua_pushinteger(L, 1);
#endif
  printf("closing\n");
  if (with_coroutine) {
    co = lua_newthread(L);
    lua_close(co);
  } else {
    lua_close(L);
  }
  printf("closed\n");
}

int main(void) {
  one_state(0);
  one_state(1);
  return 0;
}
