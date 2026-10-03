/* lua_getmetatable and lua_setmetatable on every kind of value, marking
   for __gc per dialect, and 5.1's lua_getfenv and lua_setfenv */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void pv(lua_State *L, int idx) {
  int t = lua_type(L, idx);
  switch (t) {
    case LUA_TNONE: printf("none"); break;
    case LUA_TNIL: printf("nil"); break;
    case LUA_TBOOLEAN: printf(lua_toboolean(L, idx) ? "true" : "false"); break;
    case LUA_TNUMBER:
    case LUA_TSTRING:
      lua_pushvalue(L, idx);
      printf("%s:%s", t == LUA_TNUMBER ? "n" : "s", lua_tostring(L, -1));
      lua_pop(L, 1);
      break;
    default: printf("%s", lua_typename(L, t));
  }
}

static void dostr(lua_State *L, const char *s) {
  if (luaL_loadstring(L, s) != 0 || lua_pcall(L, 0, 0, 0) != 0) {
    printf("lua error: ");
    pv(L, -1);
    printf("\n");
    lua_pop(L, 1);
  }
}

/* push a table whose __index answers "<tag>:<key>" */
static void index_mt(lua_State *L, const char *tag) {
  char src[128];
  snprintf(src, sizeof src,
           "return {__index = function(o, k) return '%s:' .. k end}", tag);
  luaL_loadstring(L, src);
  lua_call(L, 0, 1);
}

/* lua_getmetatable of the value on top: result, then pop what it pushed
   and the value */
static void getmt(lua_State *L, const char *what) {
  int top = lua_gettop(L);
  int r = lua_getmetatable(L, -1);
  printf("getmetatable %s: %d pushed=%d\n", what, r, lua_gettop(L) - top);
  lua_settop(L, top - 1);
}

/* set the metatable of the value on top to a __index table, check it
   from Lua through global `name`, and clear it again */
static void type_mt(lua_State *L, const char *name, const char *check) {
  int r;
  char src[160];
  index_mt(L, name);
  r = lua_setmetatable(L, -2);
  printf("setmetatable %s: %d top=%d\n", name, r, lua_gettop(L));
  lua_setglobal(L, name);
  snprintf(src, sizeof src, "print('%s', %s)", name, check);
  dostr(L, src);
  lua_getglobal(L, name);
  getmt(L, name);
  lua_getglobal(L, name);
  lua_pushnil(L);
  lua_setmetatable(L, -2);
  getmt(L, "cleared");
}

static void collect(lua_State *L, const char *what) {
  printf("collect %s\n", what);
  dostr(L, "collectgarbage() collectgarbage()");
}

/* a metatable with __gc printing `msg` */
static void gc_mt(lua_State *L, const char *msg) {
  char src[128];
  snprintf(src, sizeof src, "return {__gc = function() print('%s') end}", msg);
  luaL_loadstring(L, src);
  lua_call(L, 0, 1);
}

#if LUA_VERSION_NUM == 501
static int c_newud(lua_State *L) {
  lua_newuserdata(L, 4);
  return 1;
}

static void fenv(lua_State *L, const char *what) {
  lua_getfenv(L, -1);
  printf("getfenv %s: %s globals=%d mark=", what, luaL_typename(L, -1),
         lua_rawequal(L, -1, LUA_GLOBALSINDEX));
  if (lua_istable(L, -1)) {
    lua_getfield(L, -1, "mark");
    pv(L, -1);
    lua_pop(L, 1);
  }
  printf("\n");
  lua_pop(L, 1);
}

/* setfenv of the value on top to a table with mark = `mark` */
static void setfenv_mark(lua_State *L, const char *what, const char *mark) {
  int r;
  lua_newtable(L);
  lua_pushstring(L, mark);
  lua_setfield(L, -2, "mark");
  r = lua_setfenv(L, -2);
  printf("setfenv %s: %d top=%d\n", what, r, lua_gettop(L));
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  static int light;
  luaL_openlibs(L);

  lua_newtable(L);
  getmt(L, "plain table");
  lua_pushstring(L, "s");
  getmt(L, "string");
  lua_pushinteger(L, 1);
  getmt(L, "number");
  lua_pushnil(L);
  getmt(L, "nil");
  lua_pushinteger(L, 1);
  printf("getmetatable absent index: %d top=%d\n", lua_getmetatable(L, 7), lua_gettop(L));
  lua_pop(L, 1);

  lua_newtable(L);
  index_mt(L, "tbl");
  printf("setmetatable table: %d top=%d\n", lua_setmetatable(L, -2), lua_gettop(L));
  lua_getmetatable(L, -1);
  lua_getfield(L, -1, "__index");
  printf("metatable back: %s\n", luaL_typename(L, -1));
  lua_pop(L, 2);
  lua_setglobal(L, "tt");
  dostr(L, "print('tt', tt.k)");
  dostr(L, "setmetatable(tt, {__metatable = 'locked'})");
  lua_getglobal(L, "tt");
  lua_getmetatable(L, -1);
  lua_getfield(L, -1, "__metatable");
  printf("raw metatable despite __metatable: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 3);

  lua_pushinteger(L, 5);
  type_mt(L, "num", "num.x, (7).y");
  lua_pushboolean(L, 0);
  type_mt(L, "bool", "bool.x, (true).y");
  lua_pushlightuserdata(L, &light);
  type_mt(L, "light", "light.x");
  dostr(L, "co1 = coroutine.create(function() end)");
  lua_getglobal(L, "co1");
  type_mt(L, "thread", "thread.x, coroutine.create(function() end).y");
  dostr(L, "fn = function() end");
  lua_getglobal(L, "fn");
  type_mt(L, "fn", "fn.x, print.y");
  lua_pushnil(L);
  type_mt(L, "nilv", "nilv.x");

  /* finalizers: an object is marked when it gets a metatable with __gc;
     5.1 has none for tables and marks every userdata with a metatable */
  lua_newtable(L);
  gc_mt(L, "gc table");
  lua_setmetatable(L, -2);
  lua_pop(L, 1);
  collect(L, "table with __gc");
  lua_newtable(L);
  lua_newtable(L);
  lua_setmetatable(L, -2);
  lua_getmetatable(L, -1);
  gc_mt(L, "gc table late");
  lua_getfield(L, -1, "__gc");
  lua_setfield(L, -3, "__gc");
  lua_pop(L, 3);
  collect(L, "table with __gc added later");
  lua_newuserdata(L, 8);
  gc_mt(L, "gc userdata");
  lua_setmetatable(L, -2);
  lua_pop(L, 1);
  collect(L, "userdata with __gc");
  lua_newuserdata(L, 8);
  lua_newtable(L);
  lua_setmetatable(L, -2);
  lua_getmetatable(L, -1);
  gc_mt(L, "gc userdata late");
  lua_getfield(L, -1, "__gc");
  lua_setfield(L, -3, "__gc");
  lua_pop(L, 3);
  collect(L, "userdata with __gc added later");
  lua_newuserdata(L, 8);
  gc_mt(L, "never");
  lua_setmetatable(L, -2);
  lua_pushnil(L);
  lua_setmetatable(L, -2);
  lua_pop(L, 1);
  collect(L, "userdata whose metatable was removed");

#if LUA_VERSION_NUM == 501
  /* environments */
  luaL_loadstring(L, "return mark");
  fenv(L, "lua function");
  setfenv_mark(L, "lua function", "lf");
  fenv(L, "lua function");
  lua_pushvalue(L, -1);
  lua_call(L, 0, 1);
  printf("lua function sees: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 2);
  luaL_loadstring(L, "return 1");
  fenv(L, "lua function without globals");
  lua_pop(L, 1);
  lua_pushcfunction(L, c_newud);
  fenv(L, "C function");
  setfenv_mark(L, "C function", "cf");
  fenv(L, "C function");
  lua_call(L, 0, 1);
  fenv(L, "userdata made in it");
  lua_pop(L, 1);
  lua_getglobal(L, "print");
  fenv(L, "library function");
  lua_pop(L, 1);
  dostr(L, "co2 = coroutine.create(function() end)");
  lua_getglobal(L, "co2");
  fenv(L, "thread");
  setfenv_mark(L, "thread", "th");
  fenv(L, "thread");
  lua_pop(L, 1);
  lua_newuserdata(L, 4);
  fenv(L, "userdata");
  setfenv_mark(L, "userdata", "ud");
  fenv(L, "userdata");
  lua_setglobal(L, "udenv");
  dostr(L, "print('debug.getfenv', debug.getfenv(udenv).mark)");
  lua_pushinteger(L, 1);
  fenv(L, "number");
  setfenv_mark(L, "number", "n");
  lua_pop(L, 1);
#endif

  printf("final top=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
