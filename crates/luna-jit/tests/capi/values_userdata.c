/* lua_newuserdata, lua_newuserdatauv, lua_getiuservalue,
   lua_setiuservalue, lua_getuservalue and lua_setuservalue: blocks, user
   values per dialect, the debug library's view of them, and the memory a
   block counts for */
#include <stdio.h>
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

#if LUA_VERSION_NUM >= 504
static void getiuv(lua_State *L, int n) {
  int t = lua_getiuservalue(L, -1, n);
  printf("getiuservalue %d: %d ", n, t);
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
}
#endif

#if LUA_VERSION_NUM == 502 || LUA_VERSION_NUM == 503
static void getuv(lua_State *L) {
#if LUA_VERSION_NUM == 503
  int t = lua_getuservalue(L, -1);
  printf("getuservalue: %d ", t);
#else
  lua_getuservalue(L, -1);
  printf("getuservalue: ");
#endif
  pv(L, -1);
  printf(" top=%d\n", lua_gettop(L));
  lua_pop(L, 1);
}
#endif

int main(void) {
  lua_State *L = luaL_newstate();
  unsigned char *p;
  int i, ok;
  luaL_openlibs(L);

  p = (unsigned char *)lua_newuserdata(L, 16);
  for (i = 0; i < 16; i++) p[i] = (unsigned char)(i * 3);
  printf("newuserdata: %s len=%d same=%d top=%d\n", luaL_typename(L, -1), (int)RAWLEN(L, -1),
         lua_touserdata(L, -1) == (void *)p, lua_gettop(L));
  ok = 1;
  for (i = 0; i < 16; i++) ok &= ((unsigned char *)lua_touserdata(L, -1))[i] == i * 3;
  printf("block kept: %d aligned=%d\n", ok, (int)(((size_t)p) % sizeof(double) == 0));
  lua_setglobal(L, "u16");
  dostr(L, "print('type', type(u16), u16 == u16)");
  lua_newuserdata(L, 0);
  printf("newuserdata 0: %s len=%d\n", luaL_typename(L, -1), (int)RAWLEN(L, -1));
  lua_pop(L, 1);

  /* a metatable makes the block an object */
  lua_newuserdata(L, sizeof(int));
  *(int *)lua_touserdata(L, -1) = 41;
  luaL_loadstring(L, "return {__index = function(u, k) return 'ud:' .. k end,"
                     " __len = function() return 99 end}");
  lua_call(L, 0, 1);
  lua_setmetatable(L, -2);
  lua_setglobal(L, "uobj");
  dostr(L, "print('uobj', uobj.name, #uobj)");
  lua_getglobal(L, "uobj");
  printf("uobj value: %d\n", *(int *)lua_touserdata(L, -1));
  lua_pop(L, 1);

#if LUA_VERSION_NUM == 502 || LUA_VERSION_NUM == 503
  lua_newuserdata(L, 4);
  getuv(L);
  lua_newtable(L);
  lua_pushstring(L, "inuv");
  lua_setfield(L, -2, "f");
  lua_setuservalue(L, -2);
  printf("setuservalue: top=%d\n", lua_gettop(L));
  getuv(L);
  lua_pushnil(L);
  lua_setuservalue(L, -2);
  getuv(L);
#if LUA_VERSION_NUM == 503
  lua_pushinteger(L, 12);
  lua_setuservalue(L, -2);
  getuv(L);
  lua_pushstring(L, "s");
  lua_setuservalue(L, -2);
  getuv(L);
#endif
  lua_setglobal(L, "uv1");
  dostr(L, "print('debug.getuservalue', type(debug.getuservalue(uv1)))");
  dostr(L, "local t = {f = 'fromlua'} print('debug.setuservalue', debug.setuservalue(uv1, t) == uv1)");
  lua_getglobal(L, "uv1");
  lua_getuservalue(L, -1);
  lua_getfield(L, -1, "f");
  printf("set from Lua: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 3);
  /* the user value is kept alive by its userdata only */
  lua_getglobal(L, "uv1");
  lua_newtable(L);
  lua_pushstring(L, "survives");
  lua_setfield(L, -2, "f");
  lua_setuservalue(L, -2);
  lua_pop(L, 1);
  dostr(L, "collectgarbage() collectgarbage()");
  lua_getglobal(L, "uv1");
  lua_getuservalue(L, -1);
  lua_getfield(L, -1, "f");
  printf("after collect: ");
  pv(L, -1);
  printf("\n");
  lua_pop(L, 3);
#endif

#if LUA_VERSION_NUM >= 504
  lua_newuserdatauv(L, 8, 3);
  printf("newuserdatauv: %s len=%d top=%d\n", luaL_typename(L, -1), (int)RAWLEN(L, -1),
         lua_gettop(L));
  getiuv(L, 1);
  getiuv(L, 3);
  getiuv(L, 4);
  getiuv(L, 0);
  getiuv(L, -1);
  lua_pushstring(L, "two");
  printf("setiuservalue 2: %d top=%d\n", lua_setiuservalue(L, -2, 2), lua_gettop(L));
  lua_pushinteger(L, 3);
  printf("setiuservalue 3: %d\n", lua_setiuservalue(L, -2, 3));
  lua_pushstring(L, "no");
  printf("setiuservalue 4: %d top=%d\n", lua_setiuservalue(L, -2, 4), lua_gettop(L));
  lua_pushstring(L, "no");
  printf("setiuservalue 0: %d top=%d\n", lua_setiuservalue(L, -2, 0), lua_gettop(L));
  getiuv(L, 2);
  getiuv(L, 3);
  lua_newtable(L);
  lua_pushstring(L, "survives");
  lua_setfield(L, -2, "f");
  lua_setiuservalue(L, -2, 1);
  lua_setglobal(L, "uv3");
  dostr(L, "collectgarbage() collectgarbage()");
  dostr(L, "print('debug.getuservalue', debug.getuservalue(uv3, 1).f, debug.getuservalue(uv3, 2))");
  dostr(L, "print('debug.getuservalue out', select('#', debug.getuservalue(uv3, 4)), debug.getuservalue(uv3, 4))");
  dostr(L, "print('debug.setuservalue', debug.setuservalue(uv3, 'x', 3) == uv3, debug.setuservalue(uv3, 'x', 5))");
  lua_getglobal(L, "uv3");
  getiuv(L, 3);
  lua_pop(L, 1);
  lua_newuserdatauv(L, 4, 0);
  getiuv(L, 1);
  lua_pushinteger(L, 1);
  printf("setiuservalue none: %d\n", lua_setiuservalue(L, -2, 1));
  lua_pop(L, 1);
  lua_newuserdata(L, 4);
  getiuv(L, 1);
  getiuv(L, 2);
  lua_pop(L, 1);
  dostr(L, "print('file user values', select('#', debug.getuservalue(io.stdout, 1)))");
#endif

  /* the block counts in the collector's total, and stops counting once
     it is collected */
  dostr(L, "c0 = collectgarbage('count')");
  p = (unsigned char *)lua_newuserdata(L, 4 << 20);
  p[(4 << 20) - 1] = 1;
  lua_setglobal(L, "bigud");
  dostr(L, "c1 = collectgarbage('count') print('count grew', c1 - c0 >= 4096)");
  dostr(L, "bigud = nil collectgarbage() collectgarbage()"
           " print('count shrank', c1 - collectgarbage('count') >= 4096)");

  printf("final top=%d\n", lua_gettop(L));
  lua_close(L);
  return 0;
}
