/* lua_resume per dialect: what a yield, a return and an error leave on the
   thread's stack, its status after, refused resumes, restarting a thread
   at its base level, and C function bodies */
#include "threads_common.h"

static lua_State *newco(lua_State *L, const char *body) {
  lua_State *co = lua_newthread(L);
  eval(co, body);
  return co;
}

static void yields_and_returns(lua_State *L) {
  lua_State *co = newco(L,
      "return function(a, b)\n"
      "  local x, y = coroutine.yield(a + b, 'y1')\n"
      "  local z = coroutine.yield(x * y)\n"
      "  return 'done', z, a\n"
      "end");
  lua_pushinteger(co, 1);
  lua_pushinteger(co, 2);
  resume_show(co, L, 2, "first");
  /* the yielded values stay; the resume takes only its arguments */
  lua_pushinteger(co, 4);
  lua_pushinteger(co, 5);
  resume_show(co, L, 2, "second");
  lua_pop(co, 1);
  lua_pushstring(co, "zz");
  resume_show(co, L, 1, "third");
  /* at its base level again: the top value is called as a new body */
  resume_show(co, L, 0, "again");
  resume_show(co, L, 0, "dead");
  lua_settop(L, 0);
}

static void restart(lua_State *L) {
  lua_State *co = newco(L, "return function(a) return a, 'r' end");
  lua_pushstring(co, "keep");
  lua_insert(co, 1);
  lua_pushinteger(co, 5);
  resume_show(co, L, 1, "below");
  lua_settop(co, 0);
  resume_show(co, L, 0, "empty");
  lua_settop(co, 0);
  eval(co, "return function(...) return select('#', ...), ... end");
  lua_pushinteger(co, 8);
  lua_pushinteger(co, 9);
  resume_show(co, L, 2, "new body");
  lua_settop(L, 0);
}

static void errors(lua_State *L) {
  lua_State *co = newco(L, "return function(a) coroutine.yield(a); error('boom', 0) end");
  lua_pushinteger(co, 1);
  resume_show(co, L, 1, "yield");
  lua_pop(co, 1);
  printf("error: status=%d", resume(co, L, 0, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  lua_pushinteger(co, 1);
  printf("after error: status=%d", resume(co, L, 1, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  co = newco(L, "return function() local t = nil; return t.x end");
  printf("runtime error: status=%d", resume(co, L, 0, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  co = newco(L, "return function() error({}) end");
  printf("table error: status=%d", resume(co, L, 0, &(int){0}));
  printf(" lua_status=%d top=%s\n", lua_status(co), luaL_typename(co, -1));
  co = lua_newthread(L);
  lua_pushinteger(co, 3);
  printf("not a function: status=%d", resume(co, L, 0, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  lua_settop(L, 0);
}

/* a C function resuming the thread it runs on */
static int c_resume_self(lua_State *L) {
  int st;
  lua_pushstring(L, "mine");
  lua_pushinteger(L, 1);
  st = resume(L, L, 1, &(int){0});
  printf("resume self: status=%d ", st);
  show_stack(L, "frame");
  return 0;
}

/* a C function resuming the coroutine that resumed this one */
static int c_resume_global(lua_State *L) {
  lua_State *co;
  int st;
  lua_getglobal(L, "outer");
  co = lua_tothread(L, -1);
  st = resume(co, L, 0, &(int){0});
  printf("resume normal: status=%d top=[%s] lua_status=%d\n", st, lua_tostring(co, -1),
         lua_status(co));
  lua_pop(co, 1);
  return 0;
}

static void non_suspended(lua_State *L) {
  reg(L, "c_resume_self", c_resume_self);
  reg(L, "c_resume_global", c_resume_global);
  run(L, "local co = coroutine.create(function() c_resume_self(); return 'ok' end)\n"
         "print('self', coroutine.resume(co))");
  run(L, "outer = coroutine.create(function()\n"
         "  local inner = coroutine.create(function() c_resume_global() end)\n"
         "  return coroutine.resume(inner)\n"
         "end)\n"
         "print('normal', coroutine.resume(outer))");
}

#if LUA_VERSION_NUM >= 502
/* a C function body that yields without a continuation */
static int c_body(lua_State *L) {
  show_stack(L, "c_body args");
  lua_pushstring(L, "j1");
  lua_pushstring(L, "j2");
  lua_pushinteger(L, 10);
  lua_pushinteger(L, 20);
  return lua_yield(L, 2);
}
#endif

/* a C function body that errors */
static int c_err(lua_State *L) {
  lua_pushstring(L, "j");
  lua_pushstring(L, "c error");
  return lua_error(L);
}

static void c_bodies(lua_State *L) {
  lua_State *co;
  /* 5.1 cannot resume a C function body that yielded */
#if LUA_VERSION_NUM >= 502
  co = lua_newthread(L);
  lua_pushcfunction(co, c_body);
  lua_pushstring(co, "a1");
  resume_show(co, L, 1, "c yield");
  lua_pushstring(co, "r1");
  lua_pushstring(co, "r2");
  resume_show(co, L, 2, "c resumed");
#endif
  co = lua_newthread(L);
  lua_pushcfunction(co, c_err);
  lua_pushstring(co, "a1");
  printf("c error: status=%d", resume(co, L, 1, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  lua_settop(L, 0);
}

#if LUA_VERSION_NUM >= 502
/* each level resumes a new thread running this function */
static int c_deep(lua_State *L) {
  lua_State *co = lua_newthread(L);
  int st;
  lua_pushcfunction(co, c_deep);
  st = resume(co, L, 0, &(int){0});
  if (st != LUA_OK) {
    lua_xmove(co, L, 1);
    return lua_error(L);
  }
  return 0;
}

static void overflow(lua_State *L) {
  lua_State *co = lua_newthread(L);
  lua_pushcfunction(co, c_deep);
  printf("deep: status=%d", resume(co, L, 0, &(int){0}));
  printf(" top=[%s]\n", lua_tostring(co, -1));
  lua_settop(L, 0);
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  yields_and_returns(L);
  restart(L);
  errors(L);
  non_suspended(L);
  c_bodies(L);
#if LUA_VERSION_NUM >= 502
  overflow(L);
#endif
  lua_close(L);
  return 0;
}
