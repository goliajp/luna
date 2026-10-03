/* continuations: lua_yieldk and lua_callk with yields inside, nested
   continuations, a continuation yielding or calling again, errors after a
   yield, and 5.2's lua_getctx */
#include "threads_common.h"

#if LUA_VERSION_NUM >= 502

static void kshow(lua_State *L, const char *name, int status, kctx_t ctx) {
  printf("%s: status=%d ctx=%d ", name, status, (int)ctx);
  show_stack(L, "frame");
}

/* lua_yieldk from a C body; the continuation yields again */
KDEF(k_yield2) {
  KARGS;
  kshow(L, "k_yield2", status, ctx);
  lua_pushstring(L, "k2 result");
  return lua_gettop(L);
}

KDEF(k_yield1) {
  KARGS;
  kshow(L, "k_yield1", status, ctx);
  lua_pushstring(L, "again");
  return lua_yieldk(L, 1, 43, k_yield2);
}

static int c_yieldk(lua_State *L) {
#if LUA_VERSION_NUM == 502
  int ctx = -1;
  printf("getctx in function: %d ctx=%d\n", lua_getctx(L, &ctx), ctx);
#endif
  lua_pushstring(L, "junk");
  lua_pushstring(L, "y");
  return lua_yieldk(L, 1, 42, k_yield1);
}

static void yieldk_body(lua_State *L) {
  lua_State *co = lua_newthread(L);
  lua_pushcfunction(co, c_yieldk);
  lua_pushstring(co, "arg");
  resume_show(co, L, 1, "yieldk 1");
  lua_pushstring(co, "b");
  resume_show(co, L, 1, "yieldk 2");
  lua_pop(co, 1);
  lua_pushstring(co, "c");
  lua_pushstring(co, "d");
  resume_show(co, L, 2, "yieldk 3");
  lua_settop(L, 0);
}

/* lua_callk into Lua code that yields */
KDEF(k_call) {
  KARGS;
  kshow(L, "k_call", status, ctx);
  return lua_gettop(L);
}

static int c_callk(lua_State *L) {
  eval(L, "return function(a) local r = coroutine.yield('in callk', a); return 'ret', r end");
  lua_pushvalue(L, 1);
  lua_callk(L, 1, LUA_MULTRET, 7, k_call);
  printf("c_callk: no yield\n");
  return k_call(L
#if LUA_VERSION_NUM >= 503
                , LUA_OK, 7
#endif
  );
}

static void callk_body(lua_State *L) {
  lua_State *co = lua_newthread(L);
  lua_pushcfunction(co, c_callk);
  lua_pushstring(co, "x");
  resume_show(co, L, 1, "callk 1");
  lua_settop(co, 0);
  lua_pushstring(co, "resumed");
  resume_show(co, L, 1, "callk 2");
  /* the same function on the main thread cannot yield: it runs straight */
  lua_pushcfunction(L, c_callk);
  lua_pushstring(L, "main");
  if (lua_pcall(L, 1, LUA_MULTRET, 0) != LUA_OK) printf("main: %s\n", lua_tostring(L, -1));
  else show_stack(L, "main");
  lua_settop(L, 0);
}

/* nested: c_outer calls Lua, which calls c_inner, which calls Lua that
   yields; the inner continuation calls and yields again */
KDEF(k_inner2) {
  KARGS;
  kshow(L, "k_inner2", status, ctx);
  return 2;
}

KDEF(k_inner) {
  KARGS;
  kshow(L, "k_inner", status, ctx);
  eval(L, "return function() return coroutine.yield('from k_inner') end");
  lua_callk(L, 0, 1, 22, k_inner2);
  return 0;
}

static int c_inner(lua_State *L) {
  eval(L, "return function(v) return coroutine.yield('inner', v) end");
  lua_pushvalue(L, 1);
  lua_callk(L, 1, 2, 21, k_inner);
  return 0;
}

KDEF(k_outer) {
  KARGS;
  kshow(L, "k_outer", status, ctx);
  lua_pushstring(L, "outer done");
  return lua_gettop(L);
}

static int c_outer(lua_State *L) {
  eval(L, "return function() local a, b = c_inner('v'); return 'lua got', a, b end");
  lua_callk(L, 0, LUA_MULTRET, 11, k_outer);
  return 0;
}

static void nested(lua_State *L) {
  lua_State *co = lua_newthread(L);
  int i;
  reg(L, "c_inner", c_inner);
  lua_pushcfunction(co, c_outer);
  resume_show(co, L, 0, "nested 1");
  for (i = 2; i <= 4; i++) {
    char tag[16];
    lua_settop(co, 0);
    lua_pushinteger(co, i * 100);
    lua_pushinteger(co, i * 100 + 1);
    sprintf(tag, "nested %d", i);
    if (resume_show(co, L, 2, tag) != LUA_YIELD) break;
  }
  lua_settop(L, 0);
}

/* an error after a yield leaves through the C functions waiting on their
   continuations */
KDEF(k_never) {
  KARGS;
  kshow(L, "k_never", status, ctx);
  return 0;
}

static int c_err_after(lua_State *L) {
  eval(L, "return function() coroutine.yield(1); error('after yield', 0) end");
  lua_callk(L, 0, 0, 5, k_never);
  return 0;
}

KDEF(k_raises) {
  KARGS;
  kshow(L, "k_raises", status, ctx);
  lua_pushstring(L, "raised in k");
  return lua_error(L);
}

static int c_k_raises(lua_State *L) {
  return lua_yieldk(L, 0, 6, k_raises);
}

static void errors(lua_State *L) {
  lua_State *co = lua_newthread(L);
  lua_pushcfunction(co, c_err_after);
  resume_show(co, L, 0, "err 1");
  lua_settop(co, 0);
  printf("err 2: status=%d", resume(co, L, 0, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  co = lua_newthread(L);
  lua_pushcfunction(co, c_k_raises);
  resume_show(co, L, 0, "k raises 1");
  printf("k raises 2: status=%d", resume(co, L, 0, &(int){0}));
  printf(" lua_status=%d top=[%s]\n", lua_status(co), lua_tostring(co, -1));
  lua_settop(L, 0);
}

/* yields refused: across a call without a continuation, from the main
   thread, and inside a protected call without one */
static int c_call_nok(lua_State *L) {
  eval(L, "return function() coroutine.yield(1) end");
  lua_call(L, 0, 0);
  return 0;
}

static int c_pcall_nok(lua_State *L) {
  int st;
  eval(L, "return function() coroutine.yield(1) end");
  st = lua_pcall(L, 0, 0, 0);
  printf("pcall without k: status=%d msg=[%s]\n", st, lua_tostring(L, -1));
  return 0;
}

static int c_yield_main(lua_State *L) {
  return lua_yield(L, 0);
}

static void refused(lua_State *L) {
  lua_State *co = lua_newthread(L);
  lua_pushcfunction(co, c_call_nok);
  printf("call without k: status=%d", resume(co, L, 0, &(int){0}));
  printf(" top=[%s]\n", lua_tostring(co, -1));
  co = lua_newthread(L);
  lua_pushcfunction(co, c_pcall_nok);
  resume_show(co, L, 0, "pcall without k");
  lua_pushcfunction(L, c_yield_main);
  printf("yield in main: status=%d", lua_pcall(L, 0, 0, 0));
  printf(" msg=[%s]\n", lua_tostring(L, -1));
  reg(L, "c_yield_main", c_yield_main);
  run(L, "print('yield in sort', pcall(coroutine.wrap(function() table.sort({3, 2, 1}, function(a, b) c_yield_main() return a < b end) end)))");
  lua_settop(L, 0);
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  yieldk_body(L);
  callk_body(L);
  nested(L);
  errors(L);
  refused(L);
  lua_close(L);
  return 0;
}

#else
int main(void) {
  return 0;
}
#endif
