/* lua_pcallk with yields inside: success and errors after a yield, message
   handlers, an error without a yield, and nested protected calls */
#include "threads_common.h"

#if LUA_VERSION_NUM >= 502

static void kshow(lua_State *L, const char *name, int status, kctx_t ctx) {
  printf("%s: status=%d ctx=%d ", name, status, (int)ctx);
  show_stack(L, "frame");
}

KDEF(k_pcall) {
  KARGS;
  kshow(L, "k_pcall", status, ctx);
  lua_pushinteger(L, status);
  return lua_gettop(L);
}

/* upvalue 1: the Lua function to call; argument 1: no message handler (0),
   one that works (1) or one that fails (2) */
static int c_pcallk(lua_State *L) {
  int st, h = (int)lua_tointeger(L, 1);
  lua_settop(L, 0);
  lua_pushstring(L, "base");
  if (h == 1) eval(L, "return function(m) return 'handled: ' .. (type(m) == 'string' and m or type(m)) end");
  if (h == 2) eval(L, "return function(m) error('handler broke') end");
  lua_pushvalue(L, lua_upvalueindex(1));
  lua_pushstring(L, "arg");
  st = lua_pcallk(L, 1, 2, h ? 2 : 0, 9, k_pcall);
  printf("c_pcallk: returned %d\n", st);
  lua_pushinteger(L, st);
  return lua_gettop(L);
}

static void one(lua_State *L, const char *name, const char *f, int handler) {
  lua_State *co = lua_newthread(L);
  int st, i;
  printf("-- %s\n", name);
  eval(co, f);
  lua_pushcclosure(co, c_pcallk, 1);
  lua_pushinteger(co, handler);
  st = resume_show(co, L, 1, "resume 1");
  for (i = 2; st == LUA_YIELD && i < 5; i++) {
    char tag[16];
    lua_settop(co, 0);
    lua_pushinteger(co, i);
    sprintf(tag, "resume %d", i);
    st = resume_show(co, L, 1, tag);
  }
  lua_settop(L, 0);
}

/* a protected call inside the continuation of another */
KDEF(k_inner) {
  KARGS;
  kshow(L, "k_inner", status, ctx);
  return lua_gettop(L);
}

KDEF(k_outer) {
  KARGS;
  kshow(L, "k_outer", status, ctx);
  eval(L, "return function() coroutine.yield('inner'); error('inner error', 0) end");
  lua_pcallk(L, 0, 0, 0, 2, k_inner);
  printf("k_outer: no yield\n");
  return 0;
}

static int c_nested(lua_State *L) {
  eval(L, "return function() coroutine.yield('outer'); error('outer error', 0) end");
  lua_pcallk(L, 0, 0, 0, 1, k_outer);
  printf("c_nested: no yield\n");
  return 0;
}

static void nested(lua_State *L) {
  lua_State *co = lua_newthread(L);
  printf("-- nested\n");
  lua_pushcfunction(co, c_nested);
  resume_show(co, L, 0, "nested 1");
  lua_settop(co, 0);
  resume_show(co, L, 0, "nested 2");
  lua_settop(co, 0);
  resume_show(co, L, 0, "nested 3");
  lua_settop(L, 0);
}

int main(void) {
  lua_State *L = luaL_newstate();
  luaL_openlibs(L);
  printf("LUA_ERRRUN=%d LUA_ERRERR=%d\n", LUA_ERRRUN, LUA_ERRERR);
  one(L, "yield then return",
      "return function(a) local r = coroutine.yield('y', a); return 'ok', r end", 0);
  one(L, "yield then error",
      "return function(a) coroutine.yield('y'); error('late', 0) end", 0);
  one(L, "yield then error with handler",
      "return function(a) coroutine.yield('y'); error('late') end", 1);
  one(L, "yield then table error with handler",
      "return function(a) coroutine.yield('y'); error({}) end", 1);
  one(L, "error without yield",
      "return function(a) error('early', 0) end", 0);
  one(L, "error without yield with handler",
      "return function(a) error('early', 0) end", 1);
  one(L, "two yields then error",
      "return function(a) coroutine.yield(1); coroutine.yield(2); error('third', 0) end", 0);
  one(L, "handler fails after yield",
      "return function(a) coroutine.yield('y'); error('late', 0) end", 2);
  one(L, "handler fails without yield",
      "return function(a) error('early', 0) end", 2);
  nested(L);
  lua_close(L);
  return 0;
}

#else
int main(void) {
  return 0;
}
#endif
