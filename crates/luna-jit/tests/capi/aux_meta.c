/* Metatables by name, metafields, luaL_tolstring and luaL_len, references,
   luaL_gsub, the results of file and process operations, and the version
   check */
#include <errno.h>
#include "aux_common.h"

#define STR_(x) #x
#define STR(x) STR_(x)

static int f_checkudata(lua_State *L) {
  void *p = luaL_checkudata(L, 1, "Point");
  lua_pushboolean(L, p != NULL);
  return 1;
}

#if LUA_VERSION_NUM >= 502
static int f_tolstring(lua_State *L) {
  size_t len;
  const char *s = luaL_tolstring(L, 1, &len);
  lua_pushinteger(L, (lua_Integer)len);
  lua_pushboolean(L, s == lua_tostring(L, -2));
  return 3;
}

static int f_len(lua_State *L) {
  lua_pushinteger(L, (lua_Integer)luaL_len(L, 1));
  return 1;
}

static int f_version(lua_State *L) {
  lua_Number v = luaL_checknumber(L, 1);
#if LUA_VERSION_NUM == 502
  luaL_checkversion_(L, v);
#else
  luaL_checkversion_(L, v, (size_t)luaL_optinteger(L, 2, LUAL_NUMSIZES));
#endif
  lua_pushstring(L, "version ok");
  return 1;
}
#endif

static int f_tostring_mm(lua_State *L) {
  lua_pushstring(L, "from __tostring");
  return 1;
}

static void refs(lua_State *L, int t, const char *label) {
  int r1, r2, r3, r4, rn;
  int top = lua_gettop(L);
  lua_pushstring(L, "one");
  r1 = luaL_ref(L, t);
  lua_pushstring(L, "two");
  r2 = luaL_ref(L, t);
  lua_pushnil(L);
  rn = luaL_ref(L, t);
  luaL_unref(L, t, r1);
  lua_pushstring(L, "three");
  r3 = luaL_ref(L, t);
  lua_pushstring(L, "four");
  r4 = luaL_ref(L, t);
  luaL_unref(L, t, LUA_REFNIL);
  luaL_unref(L, t, LUA_NOREF);
  printf("%s refs: %d %d nil=%d reuse=%d %d top=%d\n", label, r1, r2, rn, r3, r4,
         lua_gettop(L) - top);
  lua_rawgeti(L, t, r3);
  lua_rawgeti(L, t, r2);
  lua_rawgeti(L, t, r4);
  show_from(L, "  values", top + 1);
  lua_settop(L, top);
}

int main(void) {
  lua_State *L = luaL_newstate();
  int r;
  luaL_openlibs(L);
  /* luaL_newmetatable: a new one, then the one already there */
  r = luaL_newmetatable(L, "Point");
  printf("newmetatable new=%d top=%d type=%s\n", r, lua_gettop(L), luaL_typename(L, -1));
  lua_getfield(L, -1, "__name");
  show_from(L, "  __name", 2);
  lua_settop(L, 0);
  r = luaL_newmetatable(L, "Point");
  printf("newmetatable again=%d top=%d\n", r, lua_gettop(L));
  lua_settop(L, 0);
  lua_pushinteger(L, 5);
  lua_setfield(L, LUA_REGISTRYINDEX, "Taken");
  r = luaL_newmetatable(L, "Taken");
  show_from(L, "newmetatable taken", 1);
  lua_settop(L, 0);
  luaL_getmetatable(L, "Point");
  lua_pushcfunction(L, f_tostring_mm);
  lua_setfield(L, -2, "__tostring");
  lua_pushinteger(L, 42);
  lua_setfield(L, -2, "answer");
  lua_settop(L, 0);
  /* a userdata of that type, and one of none */
  lua_newuserdata(L, 8);
#if LUA_VERSION_NUM >= 502
  luaL_setmetatable(L, "Point");
  printf("testudata point=%d", luaL_testudata(L, 1, "Point") != NULL);
  printf(" other=%d", luaL_testudata(L, 1, "Other") != NULL);
  lua_pushinteger(L, 1);
  printf(" number=%d top=%d\n", luaL_testudata(L, 2, "Point") != NULL, lua_gettop(L));
  lua_pop(L, 1);
#else
  luaL_getmetatable(L, "Point");
  lua_setmetatable(L, -2);
#endif
  lua_setglobal(L, "pt");
  lua_newuserdata(L, 8);
  lua_setglobal(L, "plain");
  lua_register(L, "cu", f_checkudata);
  dochunk(L, "print('checkudata', pcall(cu, pt))\n"
             "print('checkudata plain', pcall(cu, plain))\n"
             "print('checkudata table', pcall(cu, {}))\n"
             "print('checkudata none', pcall(cu))");
  /* metafields */
  lua_getglobal(L, "pt");
  r = luaL_getmetafield(L, 1, "answer");
  show_from(L, "getmetafield answer", 1);
  printf("  result=%d\n", r);
  lua_settop(L, 1);
  r = luaL_getmetafield(L, 1, "missing");
  printf("getmetafield missing=%d top=%d\n", r, lua_gettop(L));
  lua_pushinteger(L, 3);
  r = luaL_getmetafield(L, -1, "__index");
  printf("getmetafield number=%d top=%d\n", r, lua_gettop(L));
  lua_settop(L, 1);
  r = luaL_callmeta(L, 1, "__tostring");
  show_from(L, "callmeta", 1);
  printf("  result=%d\n", r);
  lua_settop(L, 1);
  r = luaL_callmeta(L, -1, "__nothing");
  printf("callmeta missing=%d top=%d\n", r, lua_gettop(L));
  lua_settop(L, 0);
#if LUA_VERSION_NUM >= 502
  lua_register(L, "tls", f_tolstring);
  lua_register(L, "len", f_len);
  lua_register(L, "ver", f_version);
  dochunk(L, "print('tolstring', tls(12), tls(1.5), tls(-0.0), tls(2^53))\n"
             "print('tolstring', tls('s'), tls(true), tls(false), tls(nil))\n"
             "print('tolstring mm', tls(pt))\n"
             "print('tolstring bad mm', pcall(tls, setmetatable({}, {__tostring = function() return 1 end})))\n"
             "print('tolstring num mm', pcall(tls, setmetatable({}, {__tostring = function() return 2 end})))\n"
             "print('tolstring table', (tls({}):match('^table: ')))\n"
             "print('tolstring named', (tls(setmetatable({}, {__name = 'Thing'})):match('^[^:]*')))\n"
             "print('tolstring name not string', (tls(setmetatable({}, {__name = 7})):match('^[^:]*')))\n"
             "print('tolstring function', (tls(print):match('^[^:]*')))\n"
             "print('len', len({1, 2, 3}), len('abcd'))\n"
             "print('len mm', len(setmetatable({}, {__len = function() return 9 end})))\n"
             "print('len float mm', pcall(len, setmetatable({}, {__len = function() return 2.0 end})))\n"
             "print('len bad mm', pcall(len, setmetatable({}, {__len = function() return 'x' end})))\n"
             "print('len none', pcall(len, 5))\n"
             "print('version', pcall(ver, " STR(LUA_VERSION_NUM) "))\n"
             "print('version bad', pcall(ver, 400))\n");
#if LUA_VERSION_NUM >= 503
  dochunk(L, "print('version size', pcall(ver, " STR(LUA_VERSION_NUM) ", 12))");
#endif
  /* luaL_tolstring with a relative index */
  lua_pushinteger(L, 77);
  luaL_tolstring(L, -1, NULL);
  show_from(L, "tolstring -1", 1);
  lua_settop(L, 0);
#endif
  /* references, in the registry and in a table */
  refs(L, LUA_REGISTRYINDEX, "registry");
  lua_newtable(L);
  refs(L, 1, "table");
  lua_settop(L, 0);
  /* luaL_gsub */
  printf("gsub [%s]", luaL_gsub(L, "a.b.c", ".", "::"));
  printf(" [%s]", luaL_gsub(L, "none", "x", "y"));
  printf(" [%s]", luaL_gsub(L, "aaa", "a", ""));
  printf(" [%s] top=%d\n", luaL_gsub(L, "", "a", "b"), lua_gettop(L));
  lua_settop(L, 0);
#if LUA_VERSION_NUM >= 502
  /* file and process results */
  r = luaL_fileresult(L, 1, "f");
  show_from(L, "fileresult ok", 1);
  lua_settop(L, 0);
  errno = ENOENT;
  r = luaL_fileresult(L, 0, "f");
  printf("fileresult fail n=%d top=%d nil=%d errno=%d\n", r, lua_gettop(L), lua_isnil(L, 1),
         (int)lua_tointeger(L, 3) == ENOENT);
  lua_settop(L, 0);
  errno = 0;
  r = luaL_fileresult(L, 0, NULL);
  printf("fileresult no errno n=%d code=%d\n", r, (int)lua_tointeger(L, 3));
  lua_settop(L, 0);
  errno = 0;
  r = luaL_execresult(L, 0);
  show_from(L, "execresult 0", 1);
  lua_settop(L, 0);
#if !defined(_WIN32)
  errno = 0;
  r = luaL_execresult(L, 3 << 8);
  show_from(L, "execresult 3", 1);
  lua_settop(L, 0);
  errno = 0;
  r = luaL_execresult(L, 9);
  show_from(L, "execresult signal", 1);
  lua_settop(L, 0);
#endif
  errno = 0;
  r = luaL_execresult(L, -1);
  printf("execresult -1 n=%d top=%d first=%s\n", r, lua_gettop(L), luaL_typename(L, 1));
  lua_settop(L, 0);
#endif
  printf("top at end=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
