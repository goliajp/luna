/* lua_settable, lua_setfield, lua_seti, lua_rawset, lua_rawseti,
   lua_rawsetp and lua_next: stores, metamethods, pseudo-indices, errors,
   and traversal while the table changes */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

#if LUA_VERSION_NUM == 501
#define RAWLEN lua_objlen
#else
#define RAWLEN lua_rawlen
#endif

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

static void prot(lua_State *L, const char *name, lua_CFunction f) {
  int st;
  lua_pushcfunction(L, f);
  st = lua_pcall(L, 0, 0, 0);
  printf("%s: status=%d ", name, st);
  if (st != 0) {
    pv(L, -1);
    lua_pop(L, 1);
  }
  printf(" top=%d\n", lua_gettop(L));
}

/* print the raw field k of the table on top */
static void field(lua_State *L, const char *what, const char *k) {
  lua_pushstring(L, k);
  lua_rawget(L, -2);
  printf("%s: ", what);
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
}

static void ifield(lua_State *L, const char *what, int i) {
  lua_rawgeti(L, -1, i);
  printf("%s: ", what);
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
}

static double nan_value(void) {
  volatile double z = 0.0;
  return z / z;
}

static int c_newindex(lua_State *L) {
  lua_pushvalue(L, 2);
  lua_pushstring(L, "C:");
  lua_pushvalue(L, 3);
  lua_concat(L, 2);
  lua_rawset(L, 1);
  return 0;
}

static int e_set_nil(lua_State *L) {
  lua_pushnil(L);
  lua_pushinteger(L, 1);
  lua_setfield(L, -2, "x");
  return 0;
}

static int e_set_number(lua_State *L) {
  lua_pushinteger(L, 3);
  lua_pushstring(L, "k");
  lua_pushinteger(L, 1);
  lua_settable(L, -3);
  return 0;
}

static int e_settable_nilkey(lua_State *L) {
  lua_newtable(L);
  lua_pushnil(L);
  lua_pushinteger(L, 1);
  lua_settable(L, -3);
  return 0;
}

static int e_settable_nankey(lua_State *L) {
  lua_newtable(L);
  lua_pushnumber(L, nan_value());
  lua_pushinteger(L, 1);
  lua_settable(L, -3);
  return 0;
}

static int e_rawset_nilkey(lua_State *L) {
  lua_newtable(L);
  lua_pushnil(L);
  lua_pushinteger(L, 1);
  lua_rawset(L, -3);
  return 0;
}

static int e_rawset_nankey(lua_State *L) {
  lua_newtable(L);
  lua_pushnumber(L, nan_value());
  lua_pushinteger(L, 1);
  lua_rawset(L, -3);
  return 0;
}

static int e_newindex_mm(lua_State *L) {
  lua_getglobal(L, "nerr");
  lua_pushinteger(L, 1);
  lua_setfield(L, -2, "x");
  return 0;
}

static int e_newindex_loop(lua_State *L) {
  lua_getglobal(L, "nloop");
  lua_pushinteger(L, 1);
  lua_setfield(L, -2, "x");
  return 0;
}

#if LUA_VERSION_NUM >= 503
static int e_seti_bool(lua_State *L) {
  lua_pushboolean(L, 0);
  lua_pushinteger(L, 1);
  lua_seti(L, -2, 1);
  return 0;
}
#endif

static int e_next_badkey(lua_State *L) {
  lua_newtable(L);
  lua_pushinteger(L, 1);
  lua_setfield(L, -2, "a");
  lua_pushstring(L, "nope");
  lua_next(L, -2);
  return 0;
}

static int e_next_bigkey(lua_State *L) {
  lua_newtable(L);
  lua_pushinteger(L, 1);
  lua_rawseti(L, -2, 1);
  lua_pushinteger(L, 99);
  lua_next(L, -2);
  return 0;
}

static int c_upvalues(lua_State *L) {
  lua_pushstring(L, "viaup");
  lua_setfield(L, lua_upvalueindex(1), "s");
  lua_pushstring(L, "k2");
  lua_pushstring(L, "v2");
  lua_settable(L, lua_upvalueindex(1));
  lua_pushstring(L, "raw");
  lua_rawseti(L, lua_upvalueindex(1), 3);
  printf("upvalue stores: top=%d\n", lua_gettop(L));
  lua_pushvalue(L, lua_upvalueindex(1));
  field(L, "up s", "s");
  field(L, "up k2", "k2");
  ifield(L, "up 3", 3);
  lua_pop(L, 1);
  return 0;
}

/* the key and value of a store stay on the stack while the metamethod
   collects */
static int c_gc_store(lua_State *L) {
  char key[90];
  memset(key, 'q', sizeof key - 1);
  key[sizeof key - 1] = '\0';
  lua_getglobal(L, "ngc");
  lua_pushstring(L, "a value long enough not to be a short string, surely");
  lua_setfield(L, -2, key);
  lua_pushstring(L, key);
  lua_rawget(L, -2);
  printf("gc store: %d\n", (int)RAWLEN(L, -1));
  lua_pop(L, 2);
  return 0;
}

static int cmp_str(const void *a, const void *b) {
  return strcmp(*(const char *const *)a, *(const char *const *)b);
}

/* the keys of the table on top, sorted, as text */
static void keys(lua_State *L, const char *what) {
  char buf[16][32];
  const char *ks[16];
  int n = 0, i;
  lua_pushnil(L);
  while (lua_next(L, -2) != 0) {
    lua_pushvalue(L, -2);
    snprintf(buf[n], sizeof buf[n], "%s",
             lua_isstring(L, -1) ? lua_tostring(L, -1) : luaL_typename(L, -1));
    ks[n] = buf[n];
    n++;
    lua_pop(L, 2);
  }
  qsort(ks, n, sizeof ks[0], cmp_str);
  printf("%s: n=%d", what, n);
  for (i = 0; i < n; i++) printf(" %s", ks[i]);
  printf(" top=%d\n", lua_gettop(L));
}

int main(void) {
  lua_State *L = luaL_newstate();
  static int pk;
  int n;
  luaL_openlibs(L);

  /* plain stores */
  lua_newtable(L);
  lua_pushstring(L, "k");
  lua_pushstring(L, "v");
  lua_settable(L, -3);
  printf("settable: top=%d\n", lua_gettop(L));
  lua_pushinteger(L, 12);
  lua_setfield(L, -2, "f");
  printf("setfield: top=%d\n", lua_gettop(L));
  lua_pushnumber(L, 2.0);
  lua_pushstring(L, "two");
  lua_settable(L, -3);
  lua_pushstring(L, "rk");
  lua_pushboolean(L, 1);
  lua_rawset(L, -3);
  printf("rawset: top=%d\n", lua_gettop(L));
  lua_pushstring(L, "one");
  lua_rawseti(L, -2, 1);
  printf("rawseti: top=%d\n", lua_gettop(L));
#if LUA_VERSION_NUM >= 503
  lua_pushstring(L, "three");
  lua_seti(L, -2, 3);
  printf("seti: top=%d\n", lua_gettop(L));
#endif
#if LUA_VERSION_NUM >= 502
  lua_pushstring(L, "pv");
  lua_rawsetp(L, -2, &pk);
  printf("rawsetp: top=%d\n", lua_gettop(L));
  lua_pushlightuserdata(L, &pk);
  lua_rawget(L, -2);
  printf("rawsetp back: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 1);
#else
  (void)pk;
#endif
  field(L, "k", "k");
  field(L, "f", "f");
  field(L, "rk", "rk");
  ifield(L, "1", 1);
  ifield(L, "2", 2);
  ifield(L, "3", 3);
  printf("len: %d\n", (int)RAWLEN(L, -1));
  lua_pushnil(L);
  lua_rawseti(L, -2, 2);
  ifield(L, "2 cleared", 2);
  lua_setglobal(L, "plain");

  /* __newindex: function, C function, table; rawset bypasses it, an
     existing key does not reach it */
  dostr(L, "log = {} tn = setmetatable({e = 1}, {__newindex = function(t, k, v)"
           " log[#log + 1] = tostring(k) .. '=' .. tostring(v) rawset(t, k, v) end})");
  lua_getglobal(L, "tn");
  lua_pushinteger(L, 5);
  lua_setfield(L, -2, "new");
  lua_pushinteger(L, 6);
  lua_setfield(L, -2, "e");
  lua_pushstring(L, "rawk");
  lua_pushinteger(L, 7);
  lua_rawset(L, -3);
  lua_pushinteger(L, 8);
  lua_rawseti(L, -2, 9);
  lua_pushinteger(L, 1);
  lua_pushinteger(L, 9);
  lua_settable(L, -3);
  lua_pop(L, 1);
  dostr(L, "print('log', table.concat(log, ' '), tn.new, tn.e, tn.rawk, tn[9], tn[1])");

  lua_newtable(L);
  lua_newtable(L);
  lua_pushcfunction(L, c_newindex);
  lua_setfield(L, -2, "__newindex");
  lua_setmetatable(L, -2);
  lua_pushstring(L, "w");
  lua_setfield(L, -2, "cf");
  field(L, "C __newindex", "cf");
  lua_pop(L, 1);

  dostr(L, "proxied = {} tp = setmetatable({}, {__newindex = proxied})");
  lua_getglobal(L, "tp");
  lua_pushstring(L, "pvv");
  lua_setfield(L, -2, "p");
  field(L, "table __newindex own", "p");
  lua_pop(L, 1);
  dostr(L, "print('proxied.p', proxied.p)");

  /* pseudo-indices */
  lua_newtable(L);
  lua_pushvalue(L, -1);
  lua_pushcclosure(L, c_upvalues, 1);
  lua_call(L, 0, 0);
  lua_pop(L, 1);
  lua_pushstring(L, "regv");
  lua_rawseti(L, LUA_REGISTRYINDEX, 1000);
  lua_rawgeti(L, LUA_REGISTRYINDEX, 1000);
  printf("registry rawseti: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 1);
  lua_pushstring(L, "rk");
  lua_pushstring(L, "rv");
  lua_settable(L, LUA_REGISTRYINDEX);
  lua_getfield(L, LUA_REGISTRYINDEX, "rk");
  printf("registry settable: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 1);
#if LUA_VERSION_NUM == 501
  lua_pushstring(L, "gv");
  lua_setfield(L, LUA_GLOBALSINDEX, "g1");
  lua_pushstring(L, "g2");
  lua_pushstring(L, "gv2");
  lua_settable(L, LUA_GLOBALSINDEX);
  lua_pushstring(L, "g3");
  lua_pushstring(L, "gv3");
  lua_rawset(L, LUA_GLOBALSINDEX);
  dostr(L, "print('globals', g1, g2, g3)");
#endif

  /* lua_next */
  dostr(L, "arr = {10, 20, 30}");
  lua_getglobal(L, "arr");
  lua_pushnil(L);
  while (lua_next(L, -2) != 0) {
    printf("next arr: ");
    pv(L, -2);
    printf(" ");
    pv(L, -1);
    printf(" top=%d\n", lua_gettop(L));
    lua_pop(L, 1);
  }
  printf("next end: top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
  dostr(L, "mixed = {1, 2, x = 1, y = 2, z = 3, [2.5] = 0, [true] = 1}");
  lua_getglobal(L, "mixed");
  keys(L, "mixed keys");
  lua_pop(L, 1);
  lua_newtable(L);
  lua_pushnil(L);
  printf("next empty: %d top=%d\n", lua_next(L, -2), lua_gettop(L));
  lua_pop(L, 1);

  /* clearing fields, the visited one included, while traversing */
  dostr(L, "big = {} for i = 1, 20 do big['k' .. i] = i end for i = 1, 8 do big[i] = i end");
  lua_getglobal(L, "big");
  n = 0;
  lua_pushnil(L);
  while (lua_next(L, -2) != 0) {
    n++;
    lua_pop(L, 1);
    lua_pushvalue(L, -1);
    lua_pushnil(L);
    lua_rawset(L, -4);
    if (n == 10) dostr(L, "collectgarbage()");
  }
  printf("cleared while traversing: visited=%d\n", n);
  lua_pushnil(L);
  printf("after clearing: next=%d\n", lua_next(L, -2));
  lua_pop(L, 1);

  /* the stored key and value survive a collection in __newindex */
  dostr(L, "ngc = setmetatable({}, {__newindex = function(t, k, v)"
           " collectgarbage() collectgarbage() rawset(t, k, v) end})");
  lua_pushcfunction(L, c_gc_store);
  lua_call(L, 0, 0);

  /* errors */
  dostr(L, "nerr = setmetatable({}, {__newindex = function() error('nierr', 0) end})");
  dostr(L, "nloop = {} setmetatable(nloop, {__newindex = nloop})");
  prot(L, "set nil", e_set_nil);
  prot(L, "set number", e_set_number);
  prot(L, "settable nil key", e_settable_nilkey);
  prot(L, "settable nan key", e_settable_nankey);
  prot(L, "rawset nil key", e_rawset_nilkey);
  prot(L, "rawset nan key", e_rawset_nankey);
  prot(L, "newindex mm error", e_newindex_mm);
  prot(L, "newindex loop", e_newindex_loop);
#if LUA_VERSION_NUM >= 503
  prot(L, "seti boolean", e_seti_bool);
#endif
  prot(L, "next bad key", e_next_badkey);
  prot(L, "next big key", e_next_bigkey);

  printf("final top=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
