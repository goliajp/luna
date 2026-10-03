/* lua_closethread and lua_resetthread: closing suspended, dead and fresh
   threads, pending to-be-closed variables, errors, reusing the thread */
#include "threads_common.h"

#if LUA_VERSION_NUM >= 504

#if LUA_VERSION_NUM >= 505 || LUA_VERSION_RELEASE_NUM >= 50406
#define CLOSE(co, from) lua_closethread(co, from)
#else
#define CLOSE(co, from) lua_resetthread(co)
#endif

static void close_show(lua_State *co, lua_State *from, const char *tag) {
  int st = CLOSE(co, from);
  printf("%s: close=%d lua_status=%d ", tag, st, lua_status(co));
  show_stack(co, "co");
}

static const char *tbc_body =
    "return function(tag)\n"
    "  local x <close> = setmetatable({}, {__close = function(_, e) print('__close', tag, e) end})\n"
    "  coroutine.yield('y')\n"
    "  error('dies', 0)\n"
    "end";

static int c_yield(lua_State *L) {
  lua_pushstring(L, "junk");
  lua_pushstring(L, "from C");
  return lua_yield(L, 1);
}

KDEF(k_never) {
  (void)L;
  printf("k_never: status=%d ctx=%d\n", status, (int)ctx);
  return 0;
}

static int c_yieldk(lua_State *L) {
  lua_pushstring(L, "from C k");
  return lua_yieldk(L, 1, 3, k_never);
}

/* closes the thread it runs on */
static int c_close_self(lua_State *L) {
  int st;
  printf("closing self\n");
  st = lua_closethread(L, L);
  printf("not reached %d\n", st);
  return 0;
}

int main(void) {
  lua_State *L = luaL_newstate();
  lua_State *co;
  luaL_openlibs(L);

  co = lua_newthread(L);
  lua_pushinteger(co, 1);
  lua_pushinteger(co, 2);
  close_show(co, L, "fresh with values");

  co = lua_newthread(L);
  eval(co, tbc_body);
  lua_pushstring(co, "suspended");
  resume_show(co, L, 1, "yield");
  close_show(co, L, "suspended");
  eval(co, "return function(...) return 'reused', ... end");
  lua_pushinteger(co, 5);
  resume_show(co, L, 1, "reuse");

  co = lua_newthread(L);
  eval(co, tbc_body);
  lua_pushstring(co, "dead");
  resume_show(co, L, 1, "yield");
  lua_settop(co, 0);
  resume_show(co, L, 0, "error");
  close_show(co, L, "dead");
  close_show(co, L, "again");

  co = lua_newthread(L);
  eval(co, "return function()\n"
           "  local x <close> = setmetatable({}, {__close = function() error('in close', 0) end})\n"
           "  coroutine.yield()\n"
           "end");
  resume_show(co, L, 0, "yield");
  close_show(co, L, "close errors");

  co = lua_newthread(L);
  lua_pushcfunction(co, c_yield);
  resume_show(co, L, 0, "c yield");
  close_show(co, L, "in C");
  co = lua_newthread(L);
  lua_pushcfunction(co, c_yieldk);
  resume_show(co, L, 0, "c yieldk");
  close_show(co, L, "in C with k");
  lua_pushcfunction(co, c_yield);
  resume_show(co, L, 0, "reuse after C");

  /* a coroutine closed from Lua, then resumed from C */
  co = lua_newthread(L);
  eval(co, tbc_body);
  lua_pushstring(co, "lua close");
  resume_show(co, L, 1, "yield");
  lua_setglobal(L, "cth");
  run(L, "print('coroutine.close', coroutine.close(cth), coroutine.status(cth))");
  show_stack(co, "after Lua close");
  resume_show(co, L, 0, "resume closed");

#if LUA_VERSION_NUM >= 505
  co = lua_newthread(L);
  eval(co, "return function(f)\n"
           "  local x <close> = setmetatable({}, {__close = function(_, e) print('__close self', e) end})\n"
           "  f()\n"
           "end");
  lua_pushcfunction(co, c_close_self);
  resume_show(co, L, 1, "close self");
#endif
  lua_close(L);
  return 0;
}

#else
int main(void) {
  return 0;
}
#endif
