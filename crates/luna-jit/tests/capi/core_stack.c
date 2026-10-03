/* Stack manipulation, pushes and conversions. */
#include <stdio.h>
#include <string.h>
#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

static void dump(lua_State *L, const char *what) {
  int i, n = lua_gettop(L);
  printf("%s:", what);
  for (i = 1; i <= n; i++) {
    int t = lua_type(L, i);
    if (t == LUA_TSTRING || t == LUA_TNUMBER) {
      lua_pushvalue(L, i); /* convert a copy, not the slot */
      printf(" %s", lua_tostring(L, -1));
      lua_pop(L, 1);
    } else {
      printf(" <%s>", lua_typename(L, t));
    }
  }
  printf("\n");
}

static int cfunc(lua_State *L) {
  /* upvalues through pseudo-indices, and one past the last */
  printf("upvalues: %s %s type3=%d\n", lua_tostring(L, lua_upvalueindex(1)),
         lua_tostring(L, lua_upvalueindex(2)), lua_type(L, lua_upvalueindex(3)));
  lua_pushstring(L, "changed");
  lua_replace(L, lua_upvalueindex(1));
  printf("args=%d first=%s\n", lua_gettop(L), lua_tostring(L, 1));
  lua_pushinteger(L, lua_gettop(L));
  return 1;
}

static void conversions(lua_State *L) {
  int isnum = -1;
  size_t len = 99;
  const char *s;
  lua_settop(L, 0);
  lua_pushstring(L, "  0x10  ");
  lua_pushstring(L, "3.0");
  lua_pushstring(L, "1e2");
  lua_pushstring(L, "abc");
  lua_pushnumber(L, 2.5);
  lua_pushnumber(L, -3.0);
  lua_pushinteger(L, 7);
  lua_pushboolean(L, 0);
  lua_pushnil(L);
  for (int i = 1; i <= 9; i++) {
#if LUA_VERSION_NUM >= 502
    lua_Number n = lua_tonumberx(L, i, &isnum);
#else
    lua_Number n = lua_tonumber(L, i);
    isnum = lua_isnumber(L, i);
#endif
    printf("  [%d] isnumber=%d tonumber=%.14g/%d", i, lua_isnumber(L, i), n, isnum);
#if LUA_VERSION_NUM >= 502
    {
      lua_Integer k = lua_tointegerx(L, i, &isnum);
      printf(" tointegerx=%lld/%d", (long long)k, isnum);
    }
#else
    printf(" tointeger=%lld", (long long)lua_tointeger(L, i));
#endif
    printf(" isstring=%d toboolean=%d\n", lua_isstring(L, i), lua_toboolean(L, i));
  }
  /* lua_tolstring converts a number in place */
  s = lua_tolstring(L, 5, &len);
  printf("tolstring(2.5)=%s len=%u type now=%s\n", s, (unsigned)len, luaL_typename(L, 5));
  s = lua_tolstring(L, 6, &len);
  printf("tolstring(-3.0)=%s len=%u\n", s, (unsigned)len);
  s = lua_tolstring(L, 9, &len);
  printf("tolstring(nil)=%s len=%u\n", s == NULL ? "NULL" : s, (unsigned)len);
  lua_pushlstring(L, "a\0b", 3);
  s = lua_tolstring(L, -1, &len);
  printf("embedded zero len=%u second=%d\n", (unsigned)len, s[1]);
#if LUA_VERSION_NUM >= 503
  printf("isinteger(7)=%d isinteger(2.5)=%d\n", lua_isinteger(L, 7), lua_isinteger(L, 3));
  printf("stringtonumber: %u %u", (unsigned)lua_stringtonumber(L, "0x1p4"),
         (unsigned)lua_stringtonumber(L, "nope"));
  printf(" top=%s\n", lua_tostring(L, -1));
#endif
#if LUA_VERSION_NUM == 502
  {
    lua_pushnumber(L, -1.0);
    printf("tounsignedx(-1)=%u\n", (unsigned)lua_tounsignedx(L, -1, &isnum));
    lua_pushunsigned(L, 4000000000u);
    printf("pushunsigned=%s\n", lua_tostring(L, -1));
  }
#endif
#if LUA_VERSION_NUM >= 505
  {
    char buff[LUA_N2SBUFFSZ];
    lua_pushnumber(L, 0.1);
    printf("numbertocstring=%u [%s]\n", lua_numbertocstring(L, -1, buff), buff);
    printf("numbertocstring(nil)=%u\n", (lua_pushnil(L), lua_numbertocstring(L, -1, buff)));
  }
#endif
}

int main(void) {
  lua_State *L = luaL_newstate();
  int i;
  static int marker;
  luaL_openlibs(L);
  printf("top=%d\n", lua_gettop(L));
  for (i = 1; i <= 6; i++) lua_pushinteger(L, i);
  dump(L, "pushed");
#if LUA_VERSION_NUM >= 502
  printf("absindex(-1)=%d absindex(2)=%d\n", lua_absindex(L, -1), lua_absindex(L, 2));
#endif
  lua_insert(L, 2);
  dump(L, "insert(2)");
  lua_remove(L, 1);
  dump(L, "remove(1)");
  lua_pushstring(L, "r");
  lua_replace(L, 3);
  dump(L, "replace(3)");
#if LUA_VERSION_NUM >= 503
  lua_rotate(L, 2, 2);
  dump(L, "rotate(2,2)");
  lua_rotate(L, 1, -1);
  dump(L, "rotate(1,-1)");
#endif
#if LUA_VERSION_NUM >= 502
  lua_copy(L, 1, 4);
  dump(L, "copy(1,4)");
#endif
  lua_settop(L, 8);
  dump(L, "settop(8)");
  lua_settop(L, -3);
  dump(L, "settop(-3)");
  lua_pop(L, 2);
  dump(L, "pop(2)");
  lua_pushvalue(L, -1);
  dump(L, "pushvalue(-1)");
  printf("checkstack(10)=%d checkstack(1e7)=%d\n", lua_checkstack(L, 10), lua_checkstack(L, 10000000));
  printf("type(100)=%d typename=%s\n", lua_type(L, 100), lua_typename(L, lua_type(L, 100)));
  printf("rawequal(1,1)=%d rawequal(1,100)=%d\n", lua_rawequal(L, 1, 1), lua_rawequal(L, 1, 100));
  lua_settop(L, 0);
  lua_pushlightuserdata(L, &marker);
  printf("light: type=%s isuserdata=%d same=%d\n", luaL_typename(L, 1), lua_isuserdata(L, 1),
         lua_touserdata(L, 1) == (void *)&marker);
  printf("thread is main=%d type=%s\n", lua_pushthread(L), luaL_typename(L, -1));
  printf("tothread is L=%d\n", lua_tothread(L, -1) == L);
  printf("topointer(nil)=%d topointer(thread)!=0: %d\n", lua_topointer(L, 100) == NULL,
         lua_topointer(L, -1) != NULL);
  lua_pushstring(L, "u1");
  lua_pushstring(L, "u2");
  lua_pushcclosure(L, cfunc, 2);
  printf("iscfunction=%d tocfunction=%d\n", lua_iscfunction(L, -1), lua_tocfunction(L, -1) == cfunc);
  lua_pushvalue(L, -1);
  lua_pushstring(L, "arg");
  lua_call(L, 1, 1);
  printf("call result=%s\n", lua_tostring(L, -1));
  lua_pop(L, 1);
  lua_pushstring(L, "again");
  lua_call(L, 1, 0);
  lua_getglobal(L, "print");
  printf("print iscfunction=%d\n", lua_iscfunction(L, -1));
  printf("registry type=%s\n", luaL_typename(L, LUA_REGISTRYINDEX));
#if LUA_VERSION_NUM >= 502
  printf("rawlen(\"abc\")=%u\n", (lua_pushstring(L, "abc"), (unsigned)lua_rawlen(L, -1)));
#else
  printf("objlen(12.5)=%u\n", (lua_pushnumber(L, 12.5), (unsigned)lua_objlen(L, -1)));
  printf("objlen converted: %s\n", lua_tostring(L, -1));
#endif
  conversions(L);
  lua_close(L);
  printf("closed\n");
  return 0;
}
