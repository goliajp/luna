/* the extra space of threads (5.3+): a thread starts with a copy of the
   main thread's extra space as it was when the thread was made, whether
   Lua (coroutine.create, coroutine.wrap) or C (lua_newthread) made it,
   and whenever C first sees the thread */
#include "threads_common.h"

#if LUA_VERSION_NUM >= 503

static void set_extra(lua_State *L, long v) {
  memcpy(lua_getextraspace(L), &v, sizeof v);
}

static long get_extra(lua_State *L) {
  long v;
  memcpy(&v, lua_getextraspace(L), sizeof v);
  return v;
}

/* prints the extra space of the thread it runs on */
static int extra(lua_State *L) {
  printf("  %s: extra=%ld\n", luaL_optstring(L, 1, "?"), get_extra(L));
  return 0;
}

/* sets the extra space of the thread it runs on */
static int setx(lua_State *L) {
  set_extra(L, (long)luaL_checkinteger(L, 1));
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  lua_State *t;
  luaL_openlibs(L);
  reg(L, "extra", extra);
  reg(L, "setx", setx);

  set_extra(L, 111);
  run(L, "CO = coroutine.create(function(tag) extra(tag) coroutine.yield() extra(tag) end)\n"
         "W = coroutine.wrap(function(tag) extra(tag) end)\n"
         "C2 = coroutine.create(function() end)");
  t = lua_newthread(L);
  lua_setglobal(L, "T");
  set_extra(L, 222);
  run(L, "coroutine.resume(CO, 'create')");
  run(L, "W('wrap')");
  printf("lua_newthread: extra=%ld\n", get_extra(t));
  lua_getglobal(L, "C2");
  printf("seen from C later: extra=%ld\n", get_extra(lua_tothread(L, -1)));
  lua_pop(L, 1);

  /* a thread's own changes stay its own */
  run(L, "local co = coroutine.create(function() setx(5) extra('own') coroutine.yield() extra('own') end)\n"
         "coroutine.resume(co) extra('main') coroutine.resume(co)");

  /* made inside a coroutine: still the main thread's space */
  set_extra(L, 333);
  run(L, "local outer = coroutine.create(function()\n"
         "  setx(9)\n"
         "  local inner = coroutine.create(function() extra('inner') end)\n"
         "  coroutine.resume(inner)\n"
         "end)\n"
         "coroutine.resume(outer)");
  lua_close(L);
  return 0;
}

#else
int main(void) {
  return 0;
}
#endif
