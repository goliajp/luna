/* lua_newthread, lua_pushthread, lua_tothread, lua_xmove, lua_status and
   lua_isyieldable, and threads made in C seen from Lua and the other way */
#include "threads_common.h"

static lua_State *co_seen;

#if LUA_VERSION_NUM >= 503
static int c_isyieldable(lua_State *L) {
  lua_pushinteger(L, lua_isyieldable(L));
  return 1;
}

/* a protected call without a continuation makes the callee non-yieldable */
static int c_isyieldable_in_pcall(lua_State *L) {
  lua_pushcfunction(L, c_isyieldable);
  lua_pcall(L, 0, 1, 0);
  return 1;
}
#endif

/* the running thread as C sees it from inside a function */
static int c_self(lua_State *L) {
  int ismain = lua_pushthread(L);
  printf("c_self: ismain=%d same_as_seen=%d top=%d status=%d\n", ismain,
         lua_tothread(L, -1) == L, lua_gettop(L), lua_status(L));
  co_seen = L;
  return 1;
}

static int c_body(lua_State *L) {
  int i, n = lua_gettop(L);
  printf("c_body got %d:", n);
  for (i = 1; i <= n; i++) {
    printf(" ");
    show_value(L, i);
  }
  printf("\n");
  lua_pushstring(L, "c_body done");
  return 1;
}

static void basics(lua_State *L) {
  lua_State *co;
  int r;
  r = lua_pushthread(L);
  printf("main pushthread=%d type=%s same=%d\n", r, luaL_typename(L, -1),
         lua_tothread(L, -1) == L);
  lua_pop(L, 1);
  co = lua_newthread(L);
  printf("newthread: L top=%d type=%s same=%d co top=%d status=%d\n", lua_gettop(L),
         luaL_typename(L, -1), lua_tothread(L, -1) == co, lua_gettop(co), lua_status(co));
  r = lua_pushthread(co);
  printf("co pushthread=%d co top=%d same=%d\n", r, lua_gettop(co), lua_tothread(co, -1) == co);
  lua_xmove(co, L, 1);
  printf("moved thread equal=%d L top=%d co top=%d\n", lua_rawequal(L, -1, -2),
         lua_gettop(L), lua_gettop(co));
  lua_pop(L, 1);
  lua_pushinteger(L, 1);
  lua_pushstring(L, "two");
  lua_newtable(L);
  lua_pushvalue(L, -1);
  lua_setglobal(L, "tbl");
  lua_xmove(L, co, 2);
  show_stack(co, "co after xmove");
  printf("L top=%d\n", lua_gettop(L));
  lua_xmove(co, L, 0);
  printf("after xmove 0: L top=%d co top=%d\n", lua_gettop(L), lua_gettop(co));
  lua_xmove(co, L, 2);
  lua_getglobal(L, "tbl");
  printf("table identity=%d\n", lua_rawequal(L, -1, -2));
  lua_settop(L, 0);
}

/* a thread made in C, given to Lua */
static void c_thread_in_lua(lua_State *L) {
  lua_State *co = lua_newthread(L);
  lua_setglobal(L, "cth");
  run(L, "print('empty newthread status', coroutine.status(cth))");
  run(L, "print('resume empty', coroutine.resume(cth))");
  lua_pushcfunction(co, c_body);
  lua_pushstring(co, "below");
  lua_insert(co, 1);
  printf("co top=%d\n", lua_gettop(co));
  run(L, "print('with body', coroutine.status(cth))");
  run(L, "print('resume', coroutine.resume(cth, 1, 'x'))");
  run(L, "print('after', coroutine.status(cth))");
  show_stack(co, "co after Lua resume");
  /* the same thread resumed from C with a new body */
  lua_settop(co, 0);
  eval(co, "return function(a) coroutine.yield(a + 1); return 'end' end");
  lua_pushinteger(co, 41);
  resume_show(co, L, 1, "restart");
  run(L, "print('restarted', coroutine.status(cth), coroutine.resume(cth))");
  run(L, "print('final', coroutine.status(cth))");
  lua_settop(L, 0);
}

/* a coroutine made in Lua, seen from C */
static void lua_thread_in_c(lua_State *L) {
  lua_State *co;
  eval(L, "return coroutine.create(function(a, b) local c = coroutine.yield(a + b); return c * 2 end)");
  co = lua_tothread(L, -1);
  printf("lua coroutine: status=%d ", lua_status(co));
  show_stack(co, "co");
  lua_pushinteger(co, 3);
  lua_pushinteger(co, 4);
  resume_show(co, L, 2, "resume1");
  lua_setglobal(L, "lco");
  run(L, "print('from Lua', coroutine.status(lco), coroutine.resume(lco, 21))");
  printf("after: status=%d\n", lua_status(co));
  show_stack(co, "co");
  lua_settop(L, 0);
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  basics(L);
  reg(L, "c_self", c_self);
  run(L, "local t = c_self(); print('main thread from C', t == coroutine.running())");
  run(L, "local co = coroutine.create(function() local t = c_self(); return t end)\n"
         "local ok, t = coroutine.resume(co); print('coroutine from C', ok, t == co)");
  printf("seen status=%d\n", lua_status(co_seen));
#if LUA_VERSION_NUM >= 503
  {
    lua_State *co, *co2;
    int nres;
    reg(L, "c_isyieldable", c_isyieldable);
    reg(L, "c_isyieldable_in_pcall", c_isyieldable_in_pcall);
    printf("isyieldable main=%d\n", lua_isyieldable(L));
    co = lua_newthread(L);
    printf("isyieldable fresh=%d\n", lua_isyieldable(co));
    run(L, "local co = coroutine.wrap(function() print('inside', c_isyieldable(), c_isyieldable_in_pcall()) end); co()");
    eval(L, "return coroutine.create(function() coroutine.yield() end)");
    co2 = lua_tothread(L, -1);
    printf("isyieldable lua fresh=%d\n", lua_isyieldable(co2));
    resume(co2, L, 0, &nres);
    printf("isyieldable suspended=%d\n", lua_isyieldable(co2));
    resume(co2, L, 0, &nres);
    printf("isyieldable dead=%d\n", lua_isyieldable(co2));
    lua_settop(L, 0);
  }
  {
    lua_State *a, *b;
    lua_State *co;
    *(int *)lua_getextraspace(L) = 7;
    a = lua_newthread(L);
    *(int *)lua_getextraspace(L) = 9;
    b = lua_newthread(L);
    *(int *)lua_getextraspace(a) = 11;
    printf("extraspace main=%d a=%d b=%d\n", *(int *)lua_getextraspace(L),
           *(int *)lua_getextraspace(a), *(int *)lua_getextraspace(b));
    eval(L, "return coroutine.create(function() end)");
    co = lua_tothread(L, -1);
    printf("extraspace lua coroutine=%d\n", *(int *)lua_getextraspace(co));
    lua_settop(L, 0);
  }
#endif
  c_thread_in_lua(L);
  lua_thread_in_c(L);
  lua_close(L);
  return 0;
}
