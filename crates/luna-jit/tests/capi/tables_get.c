/* lua_gettable, lua_getfield, lua_geti, lua_rawget, lua_rawgeti,
   lua_rawgetp, lua_createtable, lua_getglobal and lua_setglobal: values,
   returned types, metamethods, pseudo-indices and errors */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

/* the type a getter left on top: its return value from 5.3 on */
#if LUA_VERSION_NUM >= 503
#define RT(e) (e)
#else
#define RT(e) ((e), lua_type(L, -1))
#endif

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

/* print a getter's result: its type, the value on top, and pop it */
static void got(lua_State *L, const char *what, int t) {
  printf("%s: %s ", what, lua_typename(L, t));
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
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

static int c_index(lua_State *L) {
  lua_pushstring(L, "C:");
  lua_pushvalue(L, 2);
  lua_concat(L, 2);
  return 1;
}

static int e_index_nil(lua_State *L) {
  lua_pushnil(L);
  lua_getfield(L, -1, "x");
  return 0;
}

static int e_index_bool(lua_State *L) {
  lua_pushboolean(L, 1);
  lua_pushstring(L, "k");
  lua_gettable(L, -2);
  return 0;
}

static int e_index_mm(lua_State *L) {
  lua_getglobal(L, "terr");
  lua_getfield(L, -1, "x");
  return 0;
}

static int e_index_mm_pos(lua_State *L) {
  lua_getglobal(L, "terrpos");
  lua_getfield(L, -1, "x");
  return 0;
}

static int e_index_loop(lua_State *L) {
  lua_getglobal(L, "tloop");
  lua_getfield(L, -1, "x");
  return 0;
}

#if LUA_VERSION_NUM >= 503
static int e_geti_number(lua_State *L) {
  lua_pushinteger(L, 3);
  lua_geti(L, -1, 1);
  return 0;
}
#endif

static int e_upvalue_absent(lua_State *L) {
  lua_getfield(L, lua_upvalueindex(3), "x");
  return 0;
}

static int c_upvalues(lua_State *L) {
  int t = RT(lua_getfield(L, lua_upvalueindex(1), "x"));
  got(L, "upvalue getfield", t);
  lua_pushstring(L, "x");
  t = RT(lua_rawget(L, lua_upvalueindex(1)));
  got(L, "upvalue rawget", t);
  t = RT(lua_rawgeti(L, lua_upvalueindex(1), 1));
  got(L, "upvalue rawgeti", t);
  lua_pushinteger(L, 2);
  t = RT(lua_gettable(L, lua_upvalueindex(1)));
  got(L, "upvalue gettable", t);
  lua_pushvalue(L, lua_upvalueindex(2));
  got(L, "upvalue 2", lua_type(L, -1));
  printf("upvalue 3 type: %d\n", lua_type(L, lua_upvalueindex(3)));
  return 0;
}

/* a key built on the C side: no Lua code refers to it while the
   metamethod runs a full collection */
static int c_gc_key(lua_State *L) {
  char key[80];
  int t;
  memset(key, 'k', sizeof key - 1);
  key[sizeof key - 1] = '\0';
  lua_getglobal(L, "tgc");
  t = RT(lua_getfield(L, -1, key));
  printf("gc key: %s len=%d\n", lua_typename(L, t), (int)RAWLEN(L, -1));
  lua_pop(L, 2);
  return 0;
}

/* a yield inside a metamethod lua_getfield runs */
static int c_get_yield(lua_State *L) {
  lua_getglobal(L, "tyield");
  lua_getfield(L, -1, "x");
  lua_pushstring(L, "not reached");
  return 1;
}

#if LUA_VERSION_NUM == 501
static int c_environ(lua_State *L) {
  int t = RT(lua_getfield(L, LUA_ENVIRONINDEX, "envmark"));
  got(L, "environ getfield", t);
  return 0;
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  int t;
  static int pkey1, pkey2;
  luaL_openlibs(L);
  dostr(L, "t = setmetatable({a = 1, 'one', 'two', [2.5] = 'f'},"
           " {__index = function(t, k) return 'mm:' .. tostring(k) end})");

  lua_getglobal(L, "t");
  t = RT(lua_getfield(L, -1, "a"));
  got(L, "getfield a", t);
  t = RT(lua_getfield(L, -1, "zz"));
  got(L, "getfield zz", t);
  lua_pushinteger(L, 1);
  t = RT(lua_gettable(L, -2));
  got(L, "gettable 1", t);
  lua_pushnumber(L, 2.5);
  t = RT(lua_gettable(L, -2));
  got(L, "gettable 2.5", t);
  lua_pushnumber(L, 2.0);
  t = RT(lua_gettable(L, -2));
  got(L, "gettable 2.0", t);
  lua_pushboolean(L, 0);
  t = RT(lua_gettable(L, -2));
  got(L, "gettable false", t);
  lua_pushstring(L, "a");
  t = RT(lua_rawget(L, -2));
  got(L, "rawget a", t);
  lua_pushstring(L, "zz");
  t = RT(lua_rawget(L, -2));
  got(L, "rawget zz", t);
  lua_pushnil(L);
  t = RT(lua_rawget(L, -2));
  got(L, "rawget nil", t);
  {
    volatile double z = 0.0;
    lua_pushnumber(L, z / z);
    t = RT(lua_rawget(L, -2));
    printf("rawget nan: %s\n", lua_typename(L, t));
    lua_pop(L, 1);
  }
  t = RT(lua_rawgeti(L, -1, 1));
  got(L, "rawgeti 1", t);
  t = RT(lua_rawgeti(L, -1, 3));
  got(L, "rawgeti 3", t);
  t = RT(lua_rawgeti(L, -1, -1));
  got(L, "rawgeti -1", t);
#if LUA_VERSION_NUM >= 503
  t = lua_geti(L, -1, 2);
  got(L, "geti 2", t);
  t = lua_geti(L, -1, 7);
  got(L, "geti 7", t);
  t = lua_rawgeti(L, -1, (lua_Integer)1 << 40);
  got(L, "rawgeti 2^40", t);
#endif
  lua_pop(L, 1);

  /* __index chains: table -> table -> C function */
  lua_newtable(L);
  lua_newtable(L);
  lua_newtable(L);
  lua_pushcfunction(L, c_index);
  lua_setfield(L, -2, "__index");
  lua_setmetatable(L, -2);
  lua_pushstring(L, "mid");
  lua_setfield(L, -2, "m");
  lua_newtable(L);
  lua_pushvalue(L, -2);
  lua_setfield(L, -2, "__index");
  lua_setmetatable(L, -3);
  lua_pop(L, 1);
  t = RT(lua_getfield(L, -1, "m"));
  got(L, "chain m", t);
  t = RT(lua_getfield(L, -1, "deep"));
  got(L, "chain deep", t);
  lua_pushinteger(L, 42);
  t = RT(lua_gettable(L, -2));
  got(L, "chain 42", t);
  lua_pop(L, 1);

  /* strings index through their metatable */
  lua_pushstring(L, "abc");
  t = RT(lua_getfield(L, -1, "upper"));
  printf("string upper: %s\n", lua_typename(L, t));
  lua_pop(L, 2);

  /* light userdata keys */
#if LUA_VERSION_NUM >= 502
  lua_newtable(L);
  lua_pushstring(L, "p1");
  lua_rawsetp(L, -2, &pkey1);
  t = RT(lua_rawgetp(L, -1, &pkey1));
  got(L, "rawgetp 1", t);
  t = RT(lua_rawgetp(L, -1, &pkey2));
  got(L, "rawgetp 2", t);
  lua_pushlightuserdata(L, &pkey1);
  t = RT(lua_gettable(L, -2));
  got(L, "gettable p1", t);
  lua_pop(L, 1);
#else
  (void)pkey1;
  (void)pkey2;
#endif

  /* lua_createtable */
  lua_createtable(L, 4, 3);
  printf("createtable: %s len=%d top=%d\n", luaL_typename(L, -1), (int)RAWLEN(L, -1),
         lua_gettop(L));
  lua_pushstring(L, "v");
  lua_rawseti(L, -2, 4);
  t = RT(lua_rawgeti(L, -1, 4));
  got(L, "createtable 4", t);
  lua_pop(L, 1);
  lua_createtable(L, 0, 0);
  lua_createtable(L, 100, 0);
  printf("createtable two: top=%d\n", lua_gettop(L));
  lua_pop(L, 2);

  /* the registry */
  lua_pushstring(L, "regval");
  lua_setfield(L, LUA_REGISTRYINDEX, "luna.test");
  t = RT(lua_getfield(L, LUA_REGISTRYINDEX, "luna.test"));
  got(L, "registry getfield", t);
  lua_pushstring(L, "luna.test");
  t = RT(lua_rawget(L, LUA_REGISTRYINDEX));
  got(L, "registry rawget", t);
#if LUA_VERSION_NUM >= 502
  t = RT(lua_rawgeti(L, LUA_REGISTRYINDEX, LUA_RIDX_GLOBALS));
  lua_pushglobaltable(L);
  printf("registry globals: %s equal=%d\n", lua_typename(L, t), lua_rawequal(L, -1, -2));
  lua_pop(L, 2);
  t = RT(lua_rawgeti(L, LUA_REGISTRYINDEX, LUA_RIDX_MAINTHREAD));
  printf("registry main thread: %s\n", lua_typename(L, t));
  lua_pop(L, 1);
#else
  t = RT(lua_getfield(L, LUA_GLOBALSINDEX, "t"));
  printf("globalsindex t: %s\n", lua_typename(L, t));
  lua_pop(L, 1);
  lua_pushstring(L, "viaglobals");
  lua_setfield(L, LUA_GLOBALSINDEX, "g51");
  dostr(L, "print('g51', g51)");
  lua_pushstring(L, "t");
  t = RT(lua_rawget(L, LUA_GLOBALSINDEX));
  printf("globalsindex rawget t: %s\n", lua_typename(L, t));
  lua_pop(L, 1);
  lua_pushcfunction(L, c_environ);
  lua_pushvalue(L, -1);
  lua_call(L, 0, 0);
  lua_newtable(L);
  lua_pushstring(L, "custom env");
  lua_setfield(L, -2, "envmark");
  lua_setfenv(L, -2);
  lua_call(L, 0, 0);
#endif

  /* upvalues */
  lua_newtable(L);
  lua_pushinteger(L, 10);
  lua_setfield(L, -2, "x");
  lua_pushstring(L, "u1");
  lua_rawseti(L, -2, 1);
  lua_pushstring(L, "u2");
  lua_rawseti(L, -2, 2);
  lua_pushinteger(L, 5);
  lua_pushcclosure(L, c_upvalues, 2);
  lua_call(L, 0, 0);
  lua_pushinteger(L, 1);
  lua_pushcclosure(L, e_upvalue_absent, 1);
  printf("absent upvalue: status=%d ", lua_pcall(L, 0, 0, 0));
  pv(L, -1);
  printf("\n");
  lua_pop(L, 1);

  /* lua_getglobal and lua_setglobal */
  t = RT(lua_getglobal(L, "t"));
  printf("getglobal t: %s top=%d\n", lua_typename(L, t), lua_gettop(L));
  lua_pop(L, 1);
  t = RT(lua_getglobal(L, "nosuch"));
  got(L, "getglobal nosuch", t);
  lua_pushinteger(L, 77);
  lua_setglobal(L, "fromc");
  dostr(L, "print('fromc', fromc)");
  dostr(L, "setmetatable(_G, {__index = function(_, k) return 'G:' .. k end,"
           " __newindex = function(t, k, v) rawset(t, k, 'N:' .. tostring(v)) end})");
  t = RT(lua_getglobal(L, "undefinedx"));
  got(L, "getglobal via __index", t);
  lua_pushinteger(L, 5);
  lua_setglobal(L, "newviamm");
  dostr(L, "print('newviamm', rawget(_G, 'newviamm'))");
  dostr(L, "setmetatable(_G, nil)");
#if LUA_VERSION_NUM >= 502
  /* the globals are the registry's LUA_RIDX_GLOBALS entry */
  lua_rawgeti(L, LUA_REGISTRYINDEX, LUA_RIDX_GLOBALS);
  lua_newtable(L);
  lua_pushstring(L, "only here");
  lua_setfield(L, -2, "only");
  lua_rawseti(L, LUA_REGISTRYINDEX, LUA_RIDX_GLOBALS);
  t = RT(lua_getglobal(L, "only"));
  got(L, "replaced globals only", t);
  t = RT(lua_getglobal(L, "print"));
  got(L, "replaced globals print", t);
  lua_pushstring(L, "zv");
  lua_setglobal(L, "zset");
  lua_rawgeti(L, LUA_REGISTRYINDEX, LUA_RIDX_GLOBALS);
  t = RT(lua_getfield(L, -1, "zset"));
  got(L, "replaced globals zset", t);
  lua_pop(L, 1);
  lua_rawseti(L, LUA_REGISTRYINDEX, LUA_RIDX_GLOBALS);
  t = RT(lua_getglobal(L, "zset"));
  got(L, "restored globals zset", t);
#endif

  /* a full collection inside the metamethod keeps the key */
  dostr(L, "tgc = setmetatable({}, {__index = function(t, k)"
           " collectgarbage(); collectgarbage(); return k .. '!' end})");
  lua_pushcfunction(L, c_gc_key);
  lua_call(L, 0, 0);

  /* errors */
  dostr(L, "terr = setmetatable({}, {__index = function() error('ixerr', 0) end})");
  dostr(L, "terrpos = setmetatable({}, {__index = function() error('ixpos') end})");
  dostr(L, "tloop = {} setmetatable(tloop, {__index = tloop})");
  prot(L, "index nil", e_index_nil);
  prot(L, "index boolean", e_index_bool);
  prot(L, "index mm error", e_index_mm);
  prot(L, "index mm error pos", e_index_mm_pos);
  prot(L, "index loop", e_index_loop);
#if LUA_VERSION_NUM >= 503
  prot(L, "geti number", e_geti_number);
#endif

  /* a metamethod a C function's lua_getfield runs cannot yield */
  lua_register(L, "cget", c_get_yield);
  dostr(L, "tyield = setmetatable({}, {__index = function() coroutine.yield(1) end})");
  dostr(L, "print(coroutine.resume(coroutine.create(function() return cget() end)))");

  printf("final top=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
