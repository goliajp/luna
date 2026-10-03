/* Errors leave a C function at once: every C function below raises in the
   middle of its work, and the line after the raise must never print. */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void after(const char *who) {
  printf("  NOT REACHED: %s went on after the error\n", who);
}

static int raise_string(lua_State *L) {
  printf("  raise_string: before\n");
  lua_pushinteger(L, 1);
  lua_pushstring(L, "string error");
  lua_error(L);
  after("raise_string");
  return 0;
}

static int raise_table(lua_State *L) {
  lua_newtable(L);
  lua_pushstring(L, "inside");
  lua_setfield(L, -2, "msg");
  lua_error(L);
  after("raise_table");
  return 0;
}

static int raise_fmt(lua_State *L) {
  luaL_error(L, "formatted %s %d %f", "error", 42, 1.5);
  after("raise_fmt");
  return 0;
}

static int check_arg(lua_State *L) {
  lua_Integer n = luaL_checkinteger(L, 1);
  after("check_arg");
  lua_pushinteger(L, n);
  return 1;
}

static int call_failing(lua_State *L) {
  lua_pushvalue(L, 1);
  lua_call(L, 0, 0);
  after("call_failing");
  return 0;
}

static int index_failing(lua_State *L) {
  lua_pushstring(L, "key");
  lua_gettable(L, 1);
  after("index_failing");
  return 1;
}

static int newindex_failing(lua_State *L) {
  lua_pushinteger(L, 7);
  lua_setfield(L, 1, "field");
  after("newindex_failing");
  return 0;
}

static int concat_failing(lua_State *L) {
  lua_pushstring(L, "a");
  lua_newtable(L);
  lua_concat(L, 2);
  after("concat_failing");
  return 1;
}

static int arith_failing(lua_State *L) {
  lua_pushstring(L, "x");
  lua_pushinteger(L, 1);
#if LUA_VERSION_NUM >= 502
  lua_arith(L, LUA_OPADD);
#else
  lua_call(L, 0, 0); /* 5.1 has no lua_arith: calling a string fails too */
#endif
  after("arith_failing");
  return 1;
}

static int compare_failing(lua_State *L) {
#if LUA_VERSION_NUM >= 502
  int r = lua_compare(L, 1, 2, LUA_OPLT);
#else
  int r = lua_lessthan(L, 1, 2);
#endif
  after("compare_failing");
  lua_pushboolean(L, r);
  return 1;
}

/* catches B's error with lua_pcall and goes on */
static int catcher(lua_State *L) {
  int st;
  lua_pushcfunction(L, raise_string);
  st = lua_pcall(L, 0, 0, 0);
  printf("  catcher: pcall status=%d [%s] top=%d\n", st, lua_tostring(L, -1), lua_gettop(L));
  lua_pushstring(L, "catcher done");
  return 1;
}

/* calls a Lua function that calls a C function that raises */
static int outer(lua_State *L) {
  lua_getglobal(L, "inner_chain");
  lua_call(L, 0, 0);
  after("outer");
  return 0;
}

static int stack_left(lua_State *L) {
  int i;
  for (i = 0; i < 10; i++) lua_pushinteger(L, i);
  luaL_error(L, "with %d values pushed", lua_gettop(L));
  after("stack_left");
  return 0;
}

static int handler_raises(lua_State *L) {
  lua_pushstring(L, "handler error");
  lua_error(L);
  after("handler_raises");
  return 0;
}

#if LUA_VERSION_NUM >= 504
static int close_on_error(lua_State *L) {
  luaL_dostring(L, "return setmetatable({}, {__close = function(_, e) print('  __close got', e) end})");
  lua_toclose(L, -1);
  luaL_error(L, "after toclose");
  after("close_on_error");
  return 0;
}
#endif

static void report(lua_State *L, const char *name, int st) {
  if (lua_type(L, -1) == LUA_TTABLE) {
    lua_getfield(L, -1, "msg");
    printf("%s: status=%d table msg=%s\n", name, st, lua_tostring(L, -1));
    lua_pop(L, 1);
  } else {
    printf("%s: status=%d [%s]\n", name, st, lua_tostring(L, -1));
  }
  lua_settop(L, 0);
}

static void protect(lua_State *L, const char *name, lua_CFunction f, const char *arg_src) {
  int st, nargs = 0;
  lua_pushcfunction(L, f);
  if (arg_src != NULL) {
    nargs = luaL_dostring(L, arg_src) == 0 ? lua_gettop(L) - 1 : 0;
  }
  st = lua_pcall(L, nargs, 0, 0);
  report(L, name, st);
}

int main(void) {
  lua_State *L = luaL_newstate();
  int st;
  luaL_openlibs(L);
  lua_register(L, "raise_string", raise_string);
  lua_register(L, "raise_fmt", raise_fmt);
  lua_register(L, "check_arg", check_arg);
  lua_register(L, "outer", outer);
  lua_register(L, "handler_raises", handler_raises);
  protect(L, "raise_string", raise_string, NULL);
  protect(L, "raise_table", raise_table, NULL);
  protect(L, "raise_fmt", raise_fmt, NULL);
  protect(L, "check_arg", check_arg, "return 'not a number'");
  protect(L, "call_failing", call_failing, "return function() error('lua side', 0) end");
  protect(L, "index_failing", index_failing,
          "return setmetatable({}, {__index = function() error('index mm', 0) end})");
  protect(L, "newindex_failing", newindex_failing,
          "return setmetatable({}, {__newindex = function() error('newindex mm', 0) end})");
  protect(L, "concat_failing", concat_failing, NULL);
  protect(L, "arith_failing", arith_failing, NULL);
  protect(L, "compare_failing", compare_failing,
          "local mt = {__lt = function() error('lt mm', 0) end} return setmetatable({}, mt), setmetatable({}, mt)");
  protect(L, "stack_left", stack_left, NULL);
  lua_pushcfunction(L, catcher);
  st = lua_pcall(L, 0, 1, 0);
  report(L, "catcher", st);
  luaL_dostring(L, "function inner_chain() raise_fmt() end");
  protect(L, "outer", outer, NULL);
  /* from Lua: pcall, and the error position luaL_error adds */
  luaL_dostring(L, "print('lua pcall', pcall(raise_fmt))");
  luaL_dostring(L, "print('lua pcall check', pcall(check_arg, {}))");
  luaL_dostring(L, "print('lua xpcall', xpcall(raise_string, handler_raises))");
  luaL_dostring(L,
      "local co = coroutine.create(function() raise_string() end)\n"
      "print('coroutine', coroutine.resume(co))\n"
      "print('status', coroutine.status(co))");
#if LUA_VERSION_NUM >= 504
  protect(L, "close_on_error", close_on_error, NULL);
#endif
  printf("top at end=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
